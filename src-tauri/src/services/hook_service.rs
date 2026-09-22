//! HookService（P10 Frida 会话工作台）：js 工作目录扫描、runner CommandSpec 组装、
//! 前置检查聚合、远程端点探活。设计 docs/frida-console-design.md §3/§4/§6。
//!
//! 纪律：Command 薄层只做转发；纯逻辑（目录扫描规则、runner 参数、端点解析）
//! 分别进本文件（IO 侧）与 adapters::frida（纯参数侧）单测；
//! 脚本内容零进宿主命令行——只传绝对路径，由 runner 从磁盘读。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::Manager;

use crate::adapters::frida;
use crate::core::error::{CoreError, CoreResult};
use crate::services::config_service::{ConfigService, KEY_HOOK_WORKDIR, KEY_PYTHON_PATH};
use crate::services::env_service::EnvService;
use crate::services::process_service::CommandSpec;
use crate::services::task_service::TaskService;

/// 单次会话最多输出的事件类型对齐：hook_js_list 目录项上限（§4 防爆目录）
const JS_LIST_CAP: usize = 500;
/// 远程 frida-server 端口探活超时
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
/// runner 随包资源名（tauri.conf bundle.resources 落点）
const RUNNER_RESOURCE: &str = "scripts/frida_runner.py";

// ===== DTO =====

/// 一个可启动的本地 JS 脚本（§4 文件列表行）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsFileDto {
    pub name: String,
    pub path: String,
    pub size: i64,
    /// Unix 秒
    pub mtime: i64,
}

/// 前置检查链聚合（§3）：adb → python → frida →（远程模式）TCP 可达。
/// 前端按 ok 标红 + 按 hint 给修复跳转。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightDto {
    pub adb_ok: bool,
    pub adb_hint: Option<String>,
    pub python_ok: bool,
    pub python_hint: Option<String>,
    /// 生效的 python 解释器（runner 用它；None = 未配置/不可用）
    pub python_path: Option<String>,
    pub frida_ok: bool,
    pub frida_version: Option<String>,
    pub frida_hint: Option<String>,
    /// 远程模式探活结果；非远程模式为 None（不探）
    pub remote_ok: Option<bool>,
    pub remote_hint: Option<String>,
    /// runner 脚本是否随包可达
    pub runner_ok: bool,
    pub runner_hint: Option<String>,
}

/// hook_session_start 入参（命令层薄校验后传入）
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSessionStartArgs {
    /// USB 模式：adb serial（远程模式为 None）
    pub serial: Option<String>,
    /// 远程模式：host:port（USB 模式为 None）
    pub remote: Option<String>,
    pub spawn: bool,
    /// 包名（spawn）/ 包名或 pid（attach）
    pub target: String,
    /// 工作目录下的 .js 文件名
    pub script: String,
}

pub struct HookService {
    config: std::sync::Arc<ConfigService>,
    env: std::sync::Arc<EnvService>,
    tasks: std::sync::Arc<TaskService>,
    app: tauri::AppHandle,
}

impl HookService {
    pub fn new(
        config: std::sync::Arc<ConfigService>,
        env: std::sync::Arc<EnvService>,
        tasks: std::sync::Arc<TaskService>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            config,
            env,
            tasks,
            app,
        }
    }

    /// 已配置的工作目录（空 = 未选择）。
    pub fn workdir(&self) -> String {
        self.config
            .get(KEY_HOOK_WORKDIR, "")
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    // ===== 脚本区（§4） =====

    /// 扫描工作目录一层 *.js（非递归、忽略符号链接、上限 500、名字过白名单）。
    /// 目录非法（不存在/非目录）返回 Err，前端提示重新选择。
    pub fn list_js(&self, dir: &str) -> CoreResult<Vec<JsFileDto>> {
        scan_js_dir(Path::new(dir.trim()))
    }

    /// 选择/更新工作目录前回探：必须是存在的目录。
    pub fn validate_workdir(&self, dir: &str) -> CoreResult<()> {
        let dir = dir.trim();
        if dir.is_empty() || !Path::new(dir).is_dir() {
            return Err(CoreError::Internal(format!("工作目录不存在: {dir}")));
        }
        Ok(())
    }

    // ===== 前置检查链（§3） =====

    /// 聚合探测 adb/python/frida（§10 剪枝由 EnvService 承担：Python 未配置
    /// 不发 frida 探测子进程）。remote 非空时追加 TCP 探活。
    pub async fn preflight(&self, remote: Option<&str>) -> PreflightDto {
        let (adb, (python, frida_env)) = tokio::join!(self.env.adb(), self.env.python_and_frida());
        let (remote_ok, remote_hint) = match remote {
            Some(endpoint) => {
                let (ok, hint) = self.probe_remote(endpoint).await;
                (Some(ok), Some(hint))
            }
            None => (None, None),
        };
        let runner = self.runner_path();
        PreflightDto {
            adb_ok: adb.installed,
            adb_hint: if adb.installed {
                None
            } else {
                Some(adb.hint.unwrap_or_else(|| "adb 不可用".into()))
            },
            python_ok: python.ready,
            python_hint: python.hint.clone(),
            python_path: python.path.clone(),
            frida_ok: frida_env.installed,
            frida_version: frida_env.frida_version.clone(),
            frida_hint: frida_env.hint.clone(),
            remote_ok,
            remote_hint,
            runner_ok: runner.is_some(),
            runner_hint: if runner.is_some() {
                None
            } else {
                Some(format!(
                    "未找到随包 runner（{RUNNER_RESOURCE}）：开发环境请确认 workspace scripts/ 目录"
                ))
            },
        }
    }

    /// 探测 host:port TCP 可达（远程模式 = 端口转发是否打通）。连不上是常态。
    pub async fn probe_remote(&self, endpoint: &str) -> (bool, String) {
        let (host, port) = match frida::parse_remote_endpoint(endpoint) {
            Ok(v) => v,
            Err(e) => return (false, e.to_string()),
        };
        let addr = format!("{host}:{port}");
        use tokio::net::TcpStream;
        match tokio::time::timeout(REMOTE_PROBE_TIMEOUT, TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => (true, format!("{addr} 可达")),
            Ok(Err(e)) => (
                false,
                format!("{addr} 连接失败: {e}（去「端口转发」建 tcp:{port} → tcp:{port}）"),
            ),
            Err(_) => (
                false,
                format!("{addr} 连接超时（去「端口转发」建 tcp:{port} → tcp:{port}）"),
            ),
        }
    }

    // ===== 会话启动（§6.1/§6.2） =====

    /// 组 CommandSpec 起 runner 任务，返回 taskId（kind="frida"，输出/回放/取消
    /// 全部复用 TaskService 链路）。停止 = task_cancel，杀 runner 即 detach。
    pub async fn start_session(&self, args: HookSessionStartArgs) -> CoreResult<String> {
        let runner = self
            .runner_path()
            .ok_or_else(|| CoreError::Internal("未找到 frida runner 脚本".into()))?;
        let python = self
            .config
            .get(KEY_PYTHON_PATH, "")
            .map(|v| v.trim().to_string())
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CoreError::Internal("未配置 Python 解释器（设置 → 工具环境）".into()))?;
        if !Path::new(&python).is_file() {
            return Err(CoreError::Internal(format!(
                "Python 解释器不可用: {python}"
            )));
        }

        let script_name = args.script.trim();
        if !frida::is_safe_js_script_name(script_name) {
            return Err(CoreError::Internal(format!(
                "脚本文件名非法（仅字母数字与 _.-、不以 . 或 - 开头、.js 结尾）: {script_name}"
            )));
        }
        let workdir = self.workdir();
        if workdir.is_empty() {
            return Err(CoreError::Internal("未选择脚本工作目录".into()));
        }
        let workdir_abs = Path::new(&workdir)
            .canonicalize()
            .map_err(|_| CoreError::Internal(format!("工作目录不可访问: {workdir}")))?;
        let script_abs = workdir_abs.join(script_name);
        let script_abs = script_abs
            .canonicalize()
            .map_err(|_| CoreError::Internal(format!("脚本不存在: {}", script_abs.display())))?;
        // canonicalize 解析符号链接后必须仍落在工作目录一层内（符号链接逃逸防线）
        if script_abs.parent() != Some(workdir_abs.as_path()) || !script_abs.is_file() {
            return Err(CoreError::Internal(
                "脚本必须是所选工作目录下的普通 .js 文件（符号链接已拒绝）".into(),
            ));
        }

        let usb = args
            .serial
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let remote = args
            .remote
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if usb.is_none() && remote.is_none() {
            return Err(CoreError::Internal("必须选择设备（USB）或远程端点".into()));
        }
        if let Some(ep) = remote {
            frida::parse_remote_endpoint(ep)
                .map_err(|_| CoreError::Internal(format!("远程端点非法（host:port）: {ep}")))?;
        }
        let script_str = script_abs.to_string_lossy().into_owned();
        let runner_str = runner.to_string_lossy().into_owned();
        let spec_args = frida::build_runner_args(
            &runner_str,
            usb,
            remote,
            args.spawn,
            args.target.trim(),
            &script_str,
        )?;

        let spec = CommandSpec {
            // frida 会话不留宿主临时文件，回收点恒为空
            cleanup_paths: Vec::new(),
            executable: python.clone(),
            args: spec_args,
            cwd: Some(workdir.clone()),
            timeout: None, // 会话由用户停止（task_cancel → SIGTERM → runner detach）
            env_extra: HashMap::new(),
            hide_window: true, // Windows 防黑窗（§6.2）
        };
        // 可读会话名（§5.4）：spawn com.x · hook.js / attach 1234 · hook.js
        let mode = if args.spawn { "spawn" } else { "attach" };
        let name = format!("frida {mode} {} · {script_name}", args.target.trim());
        let task_id = self.tasks.start_with_kind_named("frida", &name, spec)?;
        tracing::info!(
            task_id = %task_id,
            mode,
            target = %args.target,
            script = %script_str,
            usb = ?usb,
            remote = ?remote,
            "frida 会话启动"
        );
        Ok(task_id)
    }

    /// runner 路径解析：① 随包资源（打包态）② workspace scripts/（dev 态）。
    /// 两处都不可得返回 None（preflight 标红）。
    fn runner_path(&self) -> Option<PathBuf> {
        if let Ok(resolved) = self
            .app
            .path()
            .resolve(RUNNER_RESOURCE, tauri::path::BaseDirectory::Resource)
        {
            if resolved.is_file() {
                return Some(resolved);
            }
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|root| root.join("scripts").join("frida_runner.py"));
        dev.filter(|p| p.is_file())
    }
}

/// 扫描规则（纯函数，单测覆盖）：一层目录、普通文件、.js 结尾、
/// 名字过白名单（隐藏/空格/元字符不进列表）、忽略符号链接、上限截断。
fn scan_js_dir(dir: &Path) -> CoreResult<Vec<JsFileDto>> {
    if !dir.is_dir() {
        return Err(CoreError::Internal(format!(
            "工作目录不存在或不是目录: {}",
            dir.display()
        )));
    }
    let read = std::fs::read_dir(dir)
        .map_err(|e| CoreError::Internal(format!("读取目录失败 {}: {e}", dir.display())))?;
    let mut out: Vec<JsFileDto> = Vec::new();
    for entry in read.flatten() {
        // 符号链接（含指向 .js 的）一律忽略：启动时 canonicalize 白名单同源防线
        let Ok(meta) = entry.file_type() else {
            continue;
        };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !frida::is_safe_js_script_name(&name) {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(JsFileDto {
            name: name.clone(),
            path: dir.join(&name).to_string_lossy().into_owned(),
            size: md.len() as i64,
            mtime,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(JS_LIST_CAP);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn scan_js_filters_non_js_dirs_hidden_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch(dir, "hook-ssl.js", "// x");
        touch(dir, "agent2.js", "// y");
        touch(dir, "notes.txt", "no");
        touch(dir, ".hidden.js", "no");
        touch(dir, "bad name.js", "no");
        std::fs::create_dir(dir.join("sub.js")).unwrap();
        let subdir = dir.join("sub.js");
        touch(&subdir, "deep.js", "// z");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("hook-ssl.js"), dir.join("link.js")).unwrap();

        let files = scan_js_dir(dir).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["agent2.js", "hook-ssl.js"],
            "只收普通 .js，按名排序"
        );
        assert!(files[0].size > 0);
        assert!(files[0].mtime > 0);
    }

    #[test]
    fn scan_js_missing_dir_is_error() {
        assert!(scan_js_dir(Path::new("/definitely/not/a/dir-xyz")).is_err());
    }

    #[test]
    fn scan_js_cap_500() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..505 {
            touch(tmp.path(), &format!("s{i:03}.js"), "//");
        }
        let files = scan_js_dir(tmp.path()).unwrap();
        assert_eq!(files.len(), JS_LIST_CAP);
    }
}
