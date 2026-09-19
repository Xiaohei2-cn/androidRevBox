//! ActivityProvider（AR6.1）：前台应用与 `/proc` 摘要在设备端完成，
//! Desktop 只消费结构化结果。
//!
//! 这一层存在的理由是消灭原来散在 Desktop 的多条 shell 字符串拼接：
//! `dumpsys package <pkg> | grep ...`、`pm list packages -3 | grep -q package:<pkg> && echo ...`、
//! `wc -l /proc/<pid>/maps` 等。这里一律改成「参数数组执行 + Rust 侧解析 + 直接读文件」，
//! 包名/PID 不再进入任何 shell 解析上下文。

use std::time::Duration;

use agent_protocol::method::ACTIVITY_FOREGROUND;
use agent_protocol::{
    ActivityForegroundParams, ActivityForegroundResult, AgentError, ErrorCode, PackageKind,
    ProcEntrySummary, ProviderHealth, ProviderInfo,
};
use serde_json::Value;
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const ACTIVITY_METHODS: &[&str] = &[ACTIVITY_FOREGROUND];
const DUMPSYS: &str = "/system/bin/dumpsys";
const PIDOF: &str = "/system/bin/pidof";
const PM: &str = "/system/bin/pm";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_CMDLINE_CHARS: usize = 120;
const STATUS_HEAD_LINES: usize = 4;

pub struct ActivityProvider;

impl ActivityProvider {
    async fn foreground(&self, params: Value) -> Result<Value, AgentError> {
        let _: ActivityForegroundParams = parse_params(params)?;

        let window = checked_stdout(
            Command::new(DUMPSYS)
                .args(["window"])
                .output()
                .await
                .map_err(|error| command_unavailable("dumpsys", error.to_string()))?,
            "dumpsys window",
        )?;
        let Some((package, activity)) = parse_foreground_window(&window) else {
            return serialize_value(ActivityForegroundResult {
                found: false,
                package_name: None,
                activity: None,
                pid: None,
                package_kind: PackageKind::Unknown,
                native_lib_dir: None,
                proc: Vec::new(),
                hint: Some("未解析到前台窗口（可能锁屏、弹窗或系统版本输出差异）".to_owned()),
            });
        };
        if !is_safe_package_name(&package) {
            return Err(AgentError::new(
                ErrorCode::Internal,
                format!("dumpsys 输出了非法包名: {package}"),
            ));
        }

        // 以下全部尽力而为：任一失败只让对应字段变成 None/Unknown，不拖垮整条前台信息。
        let pid = run(PIDOF, &[&package])
            .await
            .ok()
            .and_then(|out| parse_pidof(&out));
        let native_lib_dir = run(DUMPSYS, &["package", &package])
            .await
            .ok()
            .and_then(|out| parse_legacy_native_lib(&out));
        let package_kind = match run(PM, &["list", "packages", "-3", &package]).await {
            Ok(out) => classify_package_kind(&out, &package),
            Err(_) => PackageKind::Unknown,
        };
        let proc = match pid {
            Some(pid) => proc_summary(pid).await,
            None => Vec::new(),
        };

        serialize_value(ActivityForegroundResult {
            found: true,
            package_name: Some(package),
            activity: Some(activity),
            pid,
            package_kind,
            native_lib_dir,
            proc,
            hint: None,
        })
    }
}

impl Provider for ActivityProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "activity".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        ACTIVITY_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                ACTIVITY_FOREGROUND => self.foreground(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported activity method: {method}"),
                )),
            }
        })
    }
}

/// /proc/<pid> 摘要：直接读文件，读不到（权限或进程已退）就标 `readable=false`。
async fn proc_summary(pid: u32) -> Vec<ProcEntrySummary> {
    let maps_path = format!("/proc/{pid}/maps");
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let status_path = format!("/proc/{pid}/status");

    let (maps, cmdline, status) = tokio::join!(
        count_lines(&maps_path),
        read_to_string_lossy(&cmdline_path),
        read_head_lines(&status_path, STATUS_HEAD_LINES)
    );

    // 先取可读性再消费 Option，否则 map 之后就没法判断是否读到了
    let (maps_readable, cmdline_readable, status_readable) =
        (maps.is_some(), cmdline.is_some(), status.is_some());
    vec![
        ProcEntrySummary {
            name: "maps".into(),
            path: maps_path,
            summary: maps.map(|lines| lines.to_string()),
            readable: maps_readable,
        },
        ProcEntrySummary {
            name: "cmdline".into(),
            path: cmdline_path,
            summary: cmdline.map(|raw| {
                let text: String = raw.replace('\0', " ").trim().chars().collect();
                if text.chars().count() > MAX_CMDLINE_CHARS {
                    format!(
                        "{}…",
                        text.chars().take(MAX_CMDLINE_CHARS).collect::<String>()
                    )
                } else {
                    text
                }
            }),
            readable: cmdline_readable,
        },
        ProcEntrySummary {
            name: "status".into(),
            path: status_path,
            summary: status.map(|text| text.trim().to_owned()),
            readable: status_readable,
        },
    ]
}

async fn count_lines(path: &str) -> Option<u64> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut lines = 0_u64;
    let mut ended_with_newline = true;
    loop {
        let read = file.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                lines += 1;
                ended_with_newline = true;
            } else {
                ended_with_newline = false;
            }
        }
    }
    if !ended_with_newline {
        lines += 1;
    }
    Some(lines)
}

async fn read_to_string_lossy(path: &str) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_head_lines(path: &str, head: usize) -> Option<String> {
    let text = read_to_string_lossy(path).await?;
    Some(text.lines().take(head).collect::<Vec<_>>().join("\n"))
}

async fn run(program: &str, args: &[&str]) -> Result<String, AgentError> {
    let output = tokio::time::timeout(COMMAND_TIMEOUT, Command::new(program).args(args).output())
        .await
        .map_err(|_| AgentError::new(ErrorCode::DeadlineExceeded, "activity command timeout"))?
        .map_err(|error| command_unavailable(program, error.to_string()))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(AgentError::new(
        ErrorCode::ProviderUnavailable,
        format!("activity command failed: {program}"),
    ))
}

fn checked_stdout(output: std::process::Output, label: &str) -> Result<String, AgentError> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(AgentError::new(
        ErrorCode::ProviderUnavailable,
        format!("activity command failed: {label}"),
    )
    .with_details(serde_json::json!({ "exit_code": output.status.code() })))
}

fn command_unavailable(command: &str, reason: String) -> AgentError {
    AgentError::new(
        ErrorCode::ProviderUnavailable,
        format!("activity command unavailable: {command}"),
    )
    .with_details(serde_json::json!({ "reason": reason }))
}

/// 沿用 Desktop 侧已踩过的 ROM 兼容规则：`mCurrentFocus` 可能多行（先 null 后真实窗口），
/// 判空只看「= 之后有没有 `Window{`」，不能对整行 `contains("null")`——
/// 否则包名里含 null 的真实窗口会被误跳过。
pub fn parse_foreground_window(stdout: &str) -> Option<(String, String)> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !(trimmed.starts_with("mCurrentFocus") || trimmed.starts_with("mFocusedWindow")) {
            continue;
        }
        if !trimmed.contains('{') {
            continue;
        }
        let Some(brace) = trimmed.find('{') else {
            continue;
        };
        let inner = trimmed[brace + 1..].trim_end_matches('}');
        let Some(token) = inner.split_whitespace().find(|value| value.contains('/')) else {
            continue;
        };
        let mut parts = token.splitn(2, '/');
        let (Some(package), Some(activity)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !package.is_empty() && !activity.is_empty() {
            return Some((package.to_owned(), activity.to_owned()));
        }
    }
    None
}

pub fn parse_pidof(stdout: &str) -> Option<u32> {
    stdout
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .and_then(|value| value.parse().ok())
}

pub fn parse_legacy_native_lib(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(idx) = line.find("legacyNativeLibraryDir=") {
            let value = line[idx + "legacyNativeLibraryDir=".len()..].trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// `pm list packages -3 <pkg>` 是子串过滤，必须按整行精确匹配才算三方包。
pub fn classify_package_kind(stdout: &str, package: &str) -> PackageKind {
    let wanted = format!("package:{package}");
    if stdout
        .lines()
        .any(|line| line.trim_end_matches('\r').trim() == wanted)
    {
        PackageKind::ThirdParty
    } else {
        PackageKind::System
    }
}

/// 包名字符集白名单（防注入）。Activity 与 Package 两个 provider 共用同一份规则，
/// 免得两处对「什么叫合法包名」理解不一致（例如 framework 资源包 `android` 没有点号）。
pub(crate) fn is_safe_package_name(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 256
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid activity parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize_value<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to serialize activity result: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_focus_skipping_null_lines() {
        let out =
            "  mCurrentFocus: null\n  mCurrentFocus: Window{7a4c1de u0 com.pkg/com.pkg.Home}\n";
        assert_eq!(
            parse_foreground_window(out),
            Some(("com.pkg".into(), "com.pkg.Home".into()))
        );
        // 包名里含 null 的真实窗口不能被误判成空
        let tricky = "mCurrentFocus: Window{1 u0 com.nullpoint.app/.Main}";
        assert_eq!(
            parse_foreground_window(tricky),
            Some(("com.nullpoint.app".into(), ".Main".into()))
        );
        assert!(parse_foreground_window("mCurrentFocus: null\n").is_none());
        assert!(parse_foreground_window("nothing").is_none());
        assert!(parse_foreground_window("").is_none());
    }

    #[test]
    fn parses_pidof_and_native_lib_defensively() {
        assert_eq!(parse_pidof("4321 4322\n"), Some(4321));
        assert_eq!(parse_pidof("  77\n"), Some(77));
        assert_eq!(parse_pidof("not-a-pid"), None);
        assert_eq!(parse_pidof(""), None);
        assert_eq!(
            parse_legacy_native_lib("    legacyNativeLibraryDir=/data/app/~~x/com.y/lib/arm64\n")
                .as_deref(),
            Some("/data/app/~~x/com.y/lib/arm64")
        );
        assert_eq!(parse_legacy_native_lib("legacyNativeLibraryDir=\n"), None);
    }

    #[test]
    fn third_party_requires_exact_line_match() {
        assert_eq!(
            classify_package_kind("package:com.target.app\n", "com.target.app"),
            PackageKind::ThirdParty
        );
        // 子串过滤命中的是别的包，不能算三方
        assert_eq!(
            classify_package_kind("package:com.target.app.extra\n", "com.target.app"),
            PackageKind::System
        );
        assert_eq!(
            classify_package_kind("", "com.target.app"),
            PackageKind::System
        );
    }

    #[test]
    fn unsafe_package_names_are_rejected() {
        assert!(is_safe_package_name("com.a-b_1"));
        assert!(!is_safe_package_name("com.a;rm -rf"));
        assert!(!is_safe_package_name("com/a"));
        assert!(!is_safe_package_name(""));
    }

    #[tokio::test]
    async fn proc_summary_marks_unreadable_entries_honestly() {
        // 一个几乎肯定不存在的 pid：三项都必须 readable=false 且 summary=None，
        // 不能报 0 行或空串冒充「读到了」
        let entries = proc_summary(4_000_001).await;
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| !entry.readable));
        assert!(entries.iter().all(|entry| entry.summary.is_none()));
        assert_eq!(entries[0].name, "maps");
        assert_eq!(entries[1].name, "cmdline");
        assert_eq!(entries[2].path, "/proc/4000001/status");
    }

    #[tokio::test]
    async fn counts_lines_and_reads_head_from_real_files() {
        // 宿主可能是 macOS（没有 /proc），因此用临时文件验证读取与行数逻辑本身
        let dir = std::env::temp_dir().join(format!("activity-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with_newline = dir.join("with_nl");
        let without_newline = dir.join("no_nl");
        let path_with = with_newline.to_string_lossy().into_owned();
        let path_without = without_newline.to_string_lossy().into_owned();
        std::fs::write(
            &with_newline,
            "Name:\tapp\nState:\tS (sleeping)\nTgid:\t42\n",
        )
        .unwrap();
        std::fs::write(&without_newline, "a\0b\0c").unwrap();

        assert_eq!(count_lines(&path_with).await, Some(3));
        // 末行没有换行符时也要算一行（`wc -l` 会少算，这里不能跟着错）
        assert_eq!(count_lines(&path_without).await, Some(1));
        assert_eq!(
            read_head_lines(&path_with, 2).await.as_deref(),
            Some("Name:\tapp\nState:\tS (sleeping)")
        );
        let raw = read_to_string_lossy(&path_without).await.unwrap();
        assert_eq!(raw.replace('\0', " ").trim(), "a b c");
        // 不存在的路径必须返回 None（readable=false），不能当 0 行
        assert!(
            count_lines(&dir.join("missing").to_string_lossy())
                .await
                .is_none()
        );
        assert!(
            read_head_lines(&dir.join("missing").to_string_lossy(), 4)
                .await
                .is_none()
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
