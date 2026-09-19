//! FilesystemProvider（AR7.1）：目录/文件元数据与受限预览在设备端一次取回。
//!
//! 迁移前 Desktop 用 `adb shell ls -lA <path>` 拉文本，再在宿主侧按空格切列解析
//! （`adb::parse_ls_long`）：文件名带空格只能靠「第 8 列之后全部拼回去」猜，setuid/sticky
//! 位、uid/gid 数值、mtime 时间戳全部丢失，`total` 行与 OEM 列序差异都会静默吃掉条目；
//! 预览则是 `head -c N`/`tail -c N` 把正文当字符串传，非 UTF-8 字节会被 lossy 转换改掉，
//! 而 fifo / 字符设备（`/dev/zero`、`/proc/kmsg`）会把调用方永久挂住。
//!
//! 这里改为 `lstat` + `readlink` + `open/read` 直取：权限串由协议层 `render_mode_text`
//! 与 Legacy 同源渲染（含 s/S/t/T），时间固定 Unix epoch 秒，读不到的项如实上报
//! （`unreadable` / `readable=false` / 结构化错误），绝不把「没权限」说成「空目录/空文件」。

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use agent_protocol::method::{FILESYSTEM_LIST, FILESYSTEM_PREVIEW, FILESYSTEM_STAT};
use agent_protocol::{
    AgentError, ErrorCode, FileKind, FileStat, FilesystemListParams, FilesystemListResult,
    FilesystemPreviewParams, FilesystemPreviewResult, FilesystemStatParams, FilesystemStatResult,
    PERMISSION_BITS, PreviewEncoding, ProviderHealth, ProviderInfo, render_mode_text,
};
use serde_json::Value;

use super::{Provider, ProviderFuture, RequestContext};

const FILESYSTEM_METHODS: &[&str] = &[FILESYSTEM_LIST, FILESYSTEM_STAT, FILESYSTEM_PREVIEW];
/// 单次 `filesystem.list` 的条目上限。一帧上限 8 MiB（`MAX_FRAME_SIZE`），
/// 2 万条按每条约 200 字节算约 4 MiB：宁可标 `truncated`，也不撑爆帧、不无界扫描。
const MAX_LIST_ENTRIES: usize = 20_000;
const DEFAULT_PREVIEW_BYTES: u32 = 64 * 1024;
const MAX_PREVIEW_BYTES: u32 = 256 * 1024;

pub struct FilesystemProvider;

impl Provider for FilesystemProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "filesystem".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        FILESYSTEM_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                FILESYSTEM_LIST => self.list(params).await,
                FILESYSTEM_STAT => self.stat(params).await,
                FILESYSTEM_PREVIEW => self.preview(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported filesystem method: {method}"),
                )),
            }
        })
    }
}

impl FilesystemProvider {
    /// 列目录：`ls -lA` 的结构化替代。目录判定跟随符号链接（`/sdcard` 这类要能列），
    /// 条目本身用 lstat（指向目录的链接仍是 `symlink`，与 `ls -l` 一致）。
    async fn list(&self, params: Value) -> Result<Value, AgentError> {
        let params: FilesystemListParams = parse_params(params)?;
        let requested = validate_path(&params.path)?;
        let resolved = canonicalize(&requested)?;
        let kind = kind_of(&resolved, true)?;
        if kind != FileKind::Dir {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("不是目录: {}", resolved.display()),
            )
            .with_details(serde_json::json!({
                "reason": "not_a_directory",
                "kind": kind_name(kind),
                "path": display(&resolved),
            })));
        }
        let include_hidden = params.include_hidden;
        let target = resolved.clone();
        // 整目录扫描放进 spawn_blocking：几万个 lstat 不能在 async 上下文里同步跑完，
        // 否则一个 `/system` 就能把会话里其它请求一起拖住。
        let (entries, truncated, unreadable) =
            tokio::task::spawn_blocking(move || scan(&target, include_hidden))
                .await
                .map_err(|error| {
                    AgentError::new(ErrorCode::Internal, format!("目录扫描任务失败: {error}"))
                })?;
        serialize(FilesystemListResult {
            path: display(&resolved),
            entries,
            truncated,
            unreadable,
        })
    }

    /// 单路径元数据。`follow_symlink=false` 是 lstat 语义：只规范化父目录，
    /// 最后一节原样保留，否则「链接本身」会被目标顶掉。
    async fn stat(&self, params: Value) -> Result<Value, AgentError> {
        let params: FilesystemStatParams = parse_params(params)?;
        let requested = validate_path(&params.path)?;
        let (parent, name) = split_last(&requested);
        let resolved_parent = canonicalize(&parent)?;
        let resolved = match &name {
            Some(name) => resolved_parent.join(name),
            None => resolved_parent.clone(),
        };
        let target = if params.follow_symlink {
            match canonicalize(&resolved) {
                Ok(target) => target,
                Err(error) => return Err(with_dangling_evidence(error, &resolved)),
            }
        } else {
            resolved.clone()
        };
        let metadata = metadata_of(&target, params.follow_symlink)?;
        serialize(FilesystemStatResult {
            requested_path: params.path.clone(),
            path: display(&target),
            stat: file_stat(name.as_deref().unwrap_or("/"), &metadata, &target),
        })
    }

    /// 受限预览：替代 `head -c` / `tail -c`。
    ///
    /// 只允许普通文件——fifo、字符设备（`/dev/zero`）用 `head -c` 会永久阻塞，
    /// typed API 必须直接拒绝而不是让调用方挂死。含 NUL 或非法 UTF-8 的内容走小写 hex，
    /// 不做 lossy 文本转换（改了字节的「预览」比没有预览更危险）。
    async fn preview(&self, params: Value) -> Result<Value, AgentError> {
        let params: FilesystemPreviewParams = parse_params(params)?;
        let requested = validate_path(&params.path)?;
        let resolved = canonicalize(&requested)?;
        let kind = kind_of(&resolved, true)?;
        if kind != FileKind::File {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("预览只支持普通文件: {}", resolved.display()),
            )
            .with_details(serde_json::json!({
                "reason": "not_a_regular_file",
                "kind": kind_name(kind),
                "path": display(&resolved),
            })));
        }
        let clamped = params
            .max_bytes
            .is_some_and(|value| value > MAX_PREVIEW_BYTES);
        let limit = u64::from(
            params
                .max_bytes
                .unwrap_or(DEFAULT_PREVIEW_BYTES)
                .min(MAX_PREVIEW_BYTES),
        );
        let size = metadata_of(&resolved, true)?.size();
        let offset = if params.from_end {
            size.saturating_sub(limit)
        } else {
            0
        };
        let bytes = read_slice(&resolved, offset, limit).await?;
        let returned_bytes = u32::try_from(bytes.len()).unwrap_or(MAX_PREVIEW_BYTES);
        let (encoding, text, hex) = match std::str::from_utf8(&bytes) {
            Ok(text) if !bytes.contains(&0) => (PreviewEncoding::Utf8, Some(text.to_owned()), None),
            _ => (PreviewEncoding::Hex, None, Some(hex_encode(&bytes))),
        };
        serialize(FilesystemPreviewResult {
            path: display(&resolved),
            size,
            offset,
            returned_bytes,
            encoding,
            text,
            hex,
            // 协议语义：文件比本次返回的内容大（head 截尾、tail 截头都算）
            truncated: size > u64::from(returned_bytes),
            detail: clamped.then(|| format!("max_bytes_clamped_to_{MAX_PREVIEW_BYTES}")),
        })
    }
}

/// 读目录：先收名字、排序、再逐项 lstat。
///
/// 排序在截断之前，`truncated` 才是确定的「字典序前 N 条」，不会随 readdir 顺序漂移；
/// 逐项失败只记 `unreadable`，不影响同目录其它条目（一项 EACCES 不等于整目录失败）。
fn scan(dir: &Path, include_hidden: bool) -> (Vec<FileStat>, bool, Vec<String>) {
    let mut unreadable: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(read_dir) => {
            for item in read_dir {
                match item {
                    Ok(item) => {
                        let name = item.file_name().to_string_lossy().into_owned();
                        // `.`/`..` 永不返回（`ls -lA` 语义）
                        if name == "." || name == ".." {
                            continue;
                        }
                        if !include_hidden && name.starts_with('.') {
                            continue;
                        }
                        names.push(name);
                    }
                    Err(error) => unreadable.push(format!("<dirent>: {}", reason_of(&error))),
                }
            }
        }
        Err(error) => {
            // 能 canonicalize 却读不动（多半是缺 r 位）：如实报一条证据，
            // 不返回空列表冒充「空目录」
            unreadable.push(format!("{}: {}", display(dir), reason_of(&error)));
            return (Vec::new(), false, unreadable);
        }
    }
    let (names, truncated) = truncate_sorted(names, MAX_LIST_ENTRIES);
    let mut entries: Vec<FileStat> = Vec::with_capacity(names.len());
    for name in names {
        match std::fs::symlink_metadata(dir.join(&name)) {
            Ok(metadata) => {
                let stat = file_stat(&name, &metadata, &dir.join(&name));
                entries.push(stat);
            }
            Err(error) => unreadable.push(format!("{name}: {}", reason_of(&error))),
        }
    }
    (entries, truncated, unreadable)
}

/// 先排序再截断：与 `ls` 的字典序（LC_ALL=C）一致，也让截断结果可复现。
fn truncate_sorted(mut names: Vec<String>, max: usize) -> (Vec<String>, bool) {
    names.sort();
    let truncated = names.len() > max;
    names.truncate(max);
    (names, truncated)
}

fn file_stat(name: &str, metadata: &std::fs::Metadata, path: &Path) -> FileStat {
    let raw_mode = metadata.mode();
    let kind = FileKind::from_mode(raw_mode);
    let mode = raw_mode & PERMISSION_BITS;
    let symlink_target = (kind == FileKind::Symlink)
        .then(|| std::fs::read_link(path).ok())
        .flatten()
        // 非 UTF-8 目标按 lossy 转换（协议字段是 String）：只能用于展示，
        // 不能再拼回路径当输入
        .map(|target| target.to_string_lossy().into_owned());
    FileStat {
        name: name.to_owned(),
        kind,
        mode,
        mode_text: render_mode_text(kind, mode),
        uid: metadata.uid(),
        gid: metadata.gid(),
        size: metadata.size(),
        // 脏时钟导致的负 mtime 归 0：单位与时区固定，绝不返回本地格式字符串
        mtime_unix: metadata.mtime().max(0) as u64,
        symlink_target,
        readable: is_readable(path, kind),
    }
}

/// 当前 Agent 身份能否读内容（目录=能否列举），用 `access(2)` 按真实 uid 判定。
///
/// 注意 `access` 看不见 SELinux 拒绝，所以 `readable=true` 仍可能读失败；
/// 调用方必须把读失败当证据处理，不能退化成「空文件」。符号链接按目标判定（access 语义）。
fn is_readable(path: &Path, kind: FileKind) -> bool {
    let Ok(raw) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mode = if kind == FileKind::Dir {
        libc::R_OK | libc::X_OK
    } else {
        libc::R_OK
    };
    // SAFETY: `raw` 是本函数持有的 NUL 结尾缓冲，access 只读它。
    unsafe { libc::access(raw.as_ptr(), mode) == 0 }
}

/// 路径校验：必须绝对、必须无 `..` 段。
///
/// 「允许根」不靠人为白名单：Agent 以 shell 身份运行，真实边界是内核 DAC + SELinux，
/// 而文件浏览页现在要能走 `/`、`/sdcard`、`/data/data/<pkg>`、`/proc`，
/// 白名单只会砍掉既有能力（记 D030）。这里挡的是**输入形状**：
/// ① 相对路径会让「审计里记的路径」和「实际访问的路径」不是同一个东西；
/// ② `..` 段能把 `/data/local/tmp/../../data/data/<pkg>` 这种拼接意图藏起来。
/// 规范化后的真实路径一律回传，调用方拿到的是设备上的实际位置。
fn validate_path(raw: &str) -> Result<PathBuf, AgentError> {
    if raw.is_empty() {
        return Err(invalid_request("empty_path", "路径不能为空"));
    }
    if raw.contains('\0') {
        return Err(invalid_request("nul_in_path", "路径不得包含 NUL 字节"));
    }
    if !raw.starts_with('/') {
        return Err(invalid_request(
            "not_absolute",
            format!("路径必须是绝对路径: {raw}"),
        ));
    }
    let path = PathBuf::from(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid_request(
            "parent_escape",
            format!("路径不得包含 `..` 段: {raw}"),
        ));
    }
    Ok(normalize(&path))
}

/// 去掉 `.` 段与重复斜杠（`//a//./b` → `/a/b`）；`..` 已在 `validate_path` 拒绝。
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(part) = component {
            normalized.push(part);
        }
    }
    normalized
}

/// 拆出父目录与最后一节；根路径没有最后一节。
fn split_last(path: &Path) -> (PathBuf, Option<String>) {
    match path.file_name() {
        Some(name) => (
            path.parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
            Some(name.to_string_lossy().into_owned()),
        ),
        None => (path.to_path_buf(), None),
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, AgentError> {
    let canonical =
        std::fs::canonicalize(path).map_err(|error| io_error("canonicalize", path, error))?;
    check_allowed_roots(&canonical)?;
    Ok(canonical)
}

/// 允许根白名单（可选加固层，默认不限制）。
///
/// 默认不限制的理由：Agent 以 shell 身份运行，真实边界是内核 DAC + SELinux，而文件浏览页
/// 需要能走 `/`、`/sdcard`、`/proc`、`/data/data/<pkg>`（读不读得到由内核决定，不由我们猜）。
/// 设 `APP_REVERSE_TOOLS_AGENT_FS_ROOTS=/sdcard,/data/local/tmp` 即开启白名单：**解析符号链接
/// 之后**的真实路径必须落在其中一根之内，否则 `permission_denied` + `reason=path_not_allowed`
/// （放在 canonicalize 之后判，链接指到白名单外也逃不掉）。
///
/// ⚠️ Agent 一旦拿到 root 通道（§10 未决 / D026），这层必须默认开启，否则等价于把全盘
/// 读权限交给持有会话令牌的进程。
const ALLOWED_ROOTS_ENV: &str = "APP_REVERSE_TOOLS_AGENT_FS_ROOTS";

fn allowed_roots() -> Vec<PathBuf> {
    parse_allowed_roots(
        std::env::var(ALLOWED_ROOTS_ENV)
            .unwrap_or_default()
            .as_str(),
    )
}

/// 只接受绝对路径条目；相对路径、空串一律忽略（配置写错时宁可退化成「不限制」，
/// 也不要用一个错根把所有请求都拒掉——那会让人以为设备坏了）。
fn parse_allowed_roots(raw: &str) -> Vec<PathBuf> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| value.starts_with('/') && value.len() > 1)
        .map(PathBuf::from)
        .collect()
}

fn check_allowed_roots(canonical: &Path) -> Result<(), AgentError> {
    check_allowed_roots_with(canonical, &allowed_roots())
}

fn check_allowed_roots_with(canonical: &Path, roots: &[PathBuf]) -> Result<(), AgentError> {
    if roots.is_empty() || roots.iter().any(|root| canonical.starts_with(root)) {
        return Ok(());
    }
    Err(AgentError::new(
        ErrorCode::PermissionDenied,
        format!("路径不在允许范围内: {}", canonical.display()),
    )
    .with_details(serde_json::json!({
        "reason": "path_not_allowed",
        "path": display(canonical),
        "allowed_roots": roots.iter().map(|root| display(root)).collect::<Vec<_>>(),
    })))
}

fn metadata_of(path: &Path, follow: bool) -> Result<std::fs::Metadata, AgentError> {
    let result = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    result.map_err(|error| io_error(if follow { "stat" } else { "lstat" }, path, error))
}

fn kind_of(path: &Path, follow: bool) -> Result<FileKind, AgentError> {
    metadata_of(path, follow).map(|metadata| FileKind::from_mode(metadata.mode()))
}

/// 跟随符号链接失败时补一条证据：链接存在但目标没了（悬空链接）与「路径打错」
/// 在 UI 上是两件事，不能都只说 not_found。
fn with_dangling_evidence(error: AgentError, path: &Path) -> AgentError {
    if error.code != ErrorCode::NotFound {
        return error;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return error;
    };
    if FileKind::from_mode(metadata.mode()) != FileKind::Symlink {
        return error;
    }
    let target = std::fs::read_link(path)
        .map(|target| target.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut details = error.details.unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("dangling_symlink".into(), serde_json::json!(true));
        object.insert("symlink_target".into(), serde_json::json!(target));
    }
    AgentError::new(
        error.code,
        format!("符号链接目标不存在: {} -> {target}", path.display()),
    )
    .with_details(details)
}

async fn read_slice(path: &Path, offset: u64, limit: u64) -> Result<Vec<u8>, AgentError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| io_error("open", path, error))?;
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| io_error("seek", path, error))?;
    }
    let mut buffer = Vec::new();
    file.take(limit)
        .read_to_end(&mut buffer)
        .await
        .map_err(|error| io_error("read", path, error))?;
    Ok(buffer)
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    text
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Dir => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "symlink",
        FileKind::Socket => "socket",
        FileKind::Fifo => "fifo",
        FileKind::Block => "block",
        FileKind::Char => "char",
        FileKind::Other => "other",
    }
}

fn invalid_request(reason: &str, message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::InvalidRequest, message.into())
        .with_details(serde_json::json!({ "reason": reason }))
}

/// errno → 结构化错误码。「路径打错」「没权限」「Agent 出问题」是三种完全不同的下一步，
/// UI 要能分开，所以不能一律 internal。
fn io_error(op: &str, path: &Path, error: std::io::Error) -> AgentError {
    let errno = error.raw_os_error();
    let (code, reason) = match errno {
        Some(libc::ENOENT) => (ErrorCode::NotFound, "not_found"),
        Some(libc::ENOTDIR) => (ErrorCode::NotFound, "not_a_directory"),
        Some(libc::EACCES) | Some(libc::EPERM) => {
            (ErrorCode::PermissionDenied, "permission_denied")
        }
        Some(libc::ELOOP) => (ErrorCode::InvalidRequest, "symlink_loop"),
        Some(libc::ENAMETOOLONG) => (ErrorCode::InvalidRequest, "name_too_long"),
        _ => match error.kind() {
            std::io::ErrorKind::NotFound => (ErrorCode::NotFound, "not_found"),
            std::io::ErrorKind::PermissionDenied => {
                (ErrorCode::PermissionDenied, "permission_denied")
            }
            _ => (ErrorCode::Internal, "io_error"),
        },
    };
    AgentError::new(code, format!("{op} 失败 {}: {error}", path.display())).with_details(
        serde_json::json!({
            "reason": reason,
            "op": op,
            "path": display(path),
            "errno": errno,
            "io_kind": format!("{:?}", error.kind()),
        }),
    )
}

fn reason_of(error: &std::io::Error) -> String {
    format!("{:?} errno={:?}", error.kind(), error.raw_os_error())
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid filesystem parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to serialize filesystem result: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn json(params: serde_json::Value) -> Value {
        params
    }

    /// macOS 的 `/var` 是 `/private/var` 的符号链接：canonicalize 之后仍在允许根内，
    /// 所以这里统一用解析后的真实路径当根，避免测试只在 Linux 上成立。
    fn temp_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        (dir, canonical)
    }

    #[test]
    fn validate_path_rejects_relative_traversal_and_nul() {
        for bad in [
            "",
            "   ",
            "sdcard/Download",
            "./x",
            "/sdcard/../data/data/com.target",
            "/data/local/tmp/../../etc",
            "/sdcard/Download\0/evil",
        ] {
            let error = validate_path(bad).expect_err(&format!("{bad:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad:?}: {error:?}");
            assert!(
                error.details.is_some(),
                "{bad:?} 必须带 reason，否则 UI 无法区分「写错」和「想穿越」"
            );
        }
        let error = validate_path("/sdcard/../etc").unwrap_err();
        assert_eq!(error.details.unwrap()["reason"], "parent_escape");
    }

    #[test]
    fn validate_path_normalizes_slashes_and_dot_segments() {
        assert_eq!(
            validate_path("/sdcard/Download/").unwrap(),
            PathBuf::from("/sdcard/Download")
        );
        assert_eq!(
            validate_path("//sdcard//./x").unwrap(),
            PathBuf::from("/sdcard/x")
        );
        assert_eq!(validate_path("/").unwrap(), PathBuf::from("/"));
    }

    #[test]
    fn allowed_roots_are_opt_in_and_judged_after_resolution() {
        // 未配置 = 不限制（默认行为，文件浏览页要能走 / 与 /proc）
        assert!(parse_allowed_roots("").is_empty());
        assert!(check_allowed_roots_with(Path::new("/data/data/com.other/x"), &[]).is_ok());
        // 配置写错（相对路径/空串）时忽略该条，不当成「全部拒绝」
        assert_eq!(
            parse_allowed_roots(" sdcard , /data/local/tmp ,,"),
            vec![PathBuf::from("/data/local/tmp")]
        );
        let roots = parse_allowed_roots("/sdcard,/data/local/tmp");
        assert!(check_allowed_roots_with(Path::new("/sdcard/Download/a.txt"), &roots).is_ok());
        assert!(check_allowed_roots_with(Path::new("/data/local/tmp/x.log"), &roots).is_ok());
        let error =
            check_allowed_roots_with(Path::new("/data/data/com.other/x"), &roots).unwrap_err();
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert_eq!(error.details.unwrap()["reason"], "path_not_allowed");
        // 前缀必须按路径段比较：/data/local/tmp2 不是 /data/local/tmp 的子路径
        assert!(check_allowed_roots_with(Path::new("/data/local/tmp2/x"), &roots).is_err());
    }

    #[tokio::test]
    async fn list_returns_sorted_entries_with_metadata_and_hidden_rule() {
        let (_dir, root) = temp_root();
        std::fs::write(root.join("plain.txt"), b"hello").unwrap();
        std::fs::write(root.join(".hidden"), b"x").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::os::unix::fs::symlink("plain.txt", root.join("link")).unwrap();

        let params = |include_hidden: bool| {
            json(serde_json::json!({ "path": display(&root), "include_hidden": include_hidden }))
        };
        let value = FilesystemProvider
            .list(params(false))
            .await
            .expect("列目录应成功");
        let result: FilesystemListResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.path, display(&root), "必须回传解析后的真实路径");
        let names: Vec<&str> = result
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["link", "plain.txt", "sub"],
            "隐藏项默认不返回（ls -A 语义），且按字典序稳定排序"
        );
        assert!(!result.truncated && result.unreadable.is_empty());

        let plain = result
            .entries
            .iter()
            .find(|entry| entry.name == "plain.txt")
            .unwrap();
        assert_eq!(plain.kind, FileKind::File);
        assert_eq!(plain.size, 5);
        assert_eq!(plain.uid, unsafe { libc::getuid() });
        assert_eq!(
            plain.mode_text,
            render_mode_text(FileKind::File, plain.mode)
        );
        assert!(plain.readable, "自己写的文件应可读");
        assert!(
            plain.mtime_unix > 1_600_000_000,
            "mtime 必须是 Unix epoch 秒"
        );
        assert!(plain.symlink_target.is_none());

        let link = result
            .entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap();
        assert_eq!(
            link.kind,
            FileKind::Symlink,
            "条目用 lstat：指向文件的链接不是 file"
        );
        assert_eq!(link.symlink_target.as_deref(), Some("plain.txt"));

        let sub = result
            .entries
            .iter()
            .find(|entry| entry.name == "sub")
            .unwrap();
        assert_eq!(sub.kind, FileKind::Dir);
        assert!(sub.readable, "可列举目录 readable 应为真");

        let value = FilesystemProvider.list(params(true)).await.unwrap();
        let result: FilesystemListResult = serde_json::from_value(value).unwrap();
        assert!(
            result.entries.iter().any(|entry| entry.name == ".hidden"),
            "include_hidden=true 必须返回隐藏项"
        );
    }

    #[tokio::test]
    async fn list_distinguishes_not_a_directory_missing_and_unreadable() {
        let (_dir, root) = temp_root();
        std::fs::write(root.join("a.txt"), b"x").unwrap();

        let error = FilesystemProvider
            .list(json(
                serde_json::json!({ "path": display(&root.join("a.txt")) }),
            ))
            .await
            .expect_err("对文件列目录必须报错");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.details.unwrap()["reason"], "not_a_directory");

        let error = FilesystemProvider
            .list(json(
                serde_json::json!({ "path": display(&root.join("nope")) }),
            ))
            .await
            .expect_err("不存在的路径必须报 not_found");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.details.unwrap()["reason"], "not_found");

        // 权限不足的目录：报证据而不是空列表（root 下 DAC 不拦，故跳过）
        if unsafe { libc::geteuid() } != 0 {
            let locked = root.join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::write(locked.join("inner.txt"), b"x").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            let value = FilesystemProvider
                .list(json(serde_json::json!({ "path": display(&locked) })))
                .await
                .expect("目录本身不可读时仍应返回结构化结果");
            let result: FilesystemListResult = serde_json::from_value(value).unwrap();
            assert!(result.entries.is_empty());
            assert!(
                !result.unreadable.is_empty(),
                "读不动必须留证据，不能当成空目录"
            );
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[tokio::test]
    async fn stat_separates_link_from_target_and_flags_dangling() {
        let (_dir, root) = temp_root();
        let target = root.join("real.txt");
        std::fs::write(&target, b"12345").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let value = FilesystemProvider
            .stat(json(serde_json::json!({ "path": display(&link) })))
            .await
            .unwrap();
        let result: FilesystemStatResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.stat.kind, FileKind::Symlink);
        assert_eq!(
            result.stat.symlink_target.as_deref(),
            Some(target.to_str().unwrap())
        );
        assert_eq!(result.requested_path, display(&link));
        assert_eq!(result.stat.name, "link");

        let value = FilesystemProvider
            .stat(json(
                serde_json::json!({ "path": display(&link), "follow_symlink": true }),
            ))
            .await
            .unwrap();
        let followed: FilesystemStatResult = serde_json::from_value(value).unwrap();
        assert_eq!(followed.stat.kind, FileKind::File);
        assert_eq!(followed.stat.size, 5);
        assert!(followed.stat.symlink_target.is_none());
        assert!(followed.path.ends_with("real.txt"));

        // 悬空链接：与「路径打错」是两件事，必须带证据
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink(root.join("ghost"), &dangling).unwrap();
        let error = FilesystemProvider
            .stat(json(
                serde_json::json!({ "path": display(&dangling), "follow_symlink": true }),
            ))
            .await
            .expect_err("悬空链接必须报错");
        assert_eq!(error.code, ErrorCode::NotFound);
        let details = error.details.unwrap();
        assert_eq!(details["dangling_symlink"], serde_json::json!(true));
        assert!(
            details["symlink_target"]
                .as_str()
                .unwrap()
                .ends_with("ghost")
        );
    }

    #[tokio::test]
    async fn preview_caps_bytes_uses_hex_for_binary_and_supports_tail() {
        let (_dir, root) = temp_root();
        let text = root.join("a.log");
        std::fs::write(&text, "0123456789".repeat(20)).unwrap();

        let value = FilesystemProvider
            .preview(json(
                serde_json::json!({ "path": display(&text), "max_bytes": 8 }),
            ))
            .await
            .unwrap();
        let head: FilesystemPreviewResult = serde_json::from_value(value).unwrap();
        assert_eq!(head.encoding, PreviewEncoding::Utf8);
        assert_eq!(head.text.as_deref(), Some("01234567"));
        assert_eq!((head.offset, head.returned_bytes, head.size), (0, 8, 200));
        assert!(head.truncated, "文件比返回内容大时必须标截断");
        assert!(head.hex.is_none());

        let value = FilesystemProvider
            .preview(json(serde_json::json!({
                "path": display(&text), "max_bytes": 10, "from_end": true
            })))
            .await
            .unwrap();
        let tail: FilesystemPreviewResult = serde_json::from_value(value).unwrap();
        assert_eq!(tail.offset, 190, "尾读必须给出真实偏移");
        assert_eq!(tail.text.as_deref(), Some("0123456789"));

        let binary = root.join("lib.so");
        std::fs::write(&binary, [0x7f, b'E', b'L', b'F', 0x00, 0xff]).unwrap();
        let value = FilesystemProvider
            .preview(json(serde_json::json!({ "path": display(&binary) })))
            .await
            .unwrap();
        let result: FilesystemPreviewResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.encoding, PreviewEncoding::Hex);
        assert_eq!(result.hex.as_deref(), Some("7f454c4600ff"));
        assert!(result.text.is_none(), "二进制不得做 lossy 文本转换");

        let value = FilesystemProvider
            .preview(json(serde_json::json!({
                "path": display(&binary), "max_bytes": 10 * 1024 * 1024
            })))
            .await
            .unwrap();
        let clamped: FilesystemPreviewResult = serde_json::from_value(value).unwrap();
        assert!(
            clamped
                .detail
                .unwrap()
                .contains(&format!("{MAX_PREVIEW_BYTES}")),
            "超过硬上限必须显式告知被夹到多少"
        );
    }

    /// fifo / 字符设备用 `head -c` 会永久阻塞：typed API 必须直接拒，不能挂死会话。
    #[tokio::test]
    async fn preview_refuses_directories_and_fifos() {
        let (_dir, root) = temp_root();
        let error = FilesystemProvider
            .preview(json(serde_json::json!({ "path": display(&root) })))
            .await
            .expect_err("目录不能预览");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.details.unwrap()["reason"], "not_a_regular_file");

        let fifo = root.join("pipe");
        let raw = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: raw 是本函数持有的 NUL 结尾路径，mkfifo 只写这个路径。
        let created = unsafe { libc::mkfifo(raw.as_ptr(), 0o600) };
        if created != 0 {
            return; // 平台不支持 fifo 时跳过，不制造假失败
        }
        let fifo_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            FilesystemProvider.preview(json(serde_json::json!({ "path": display(&fifo) }))),
        )
        .await
        .expect("fifo 预览必须立刻返回而不是阻塞");
        let error = fifo_result.expect_err("fifo 不能预览");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.details.unwrap()["kind"], "fifo");
    }

    #[test]
    fn truncate_sorted_is_deterministic_and_flags_truncation() {
        let names = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let (kept, truncated) = truncate_sorted(names.clone(), 2);
        assert_eq!(
            kept,
            ["a", "b"],
            "截断必须取字典序前 N 条，不能随 readdir 顺序漂移"
        );
        assert!(truncated);
        let (kept, truncated) = truncate_sorted(names, 10);
        assert_eq!(kept, ["a", "b", "c"]);
        assert!(!truncated);
    }

    #[test]
    fn hex_encode_is_lowercase_and_complete() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xff, 0x7f]), "000fff7f");
    }

    #[test]
    fn mode_text_renders_setuid_sticky_like_ls() {
        // /system/bin 下常见 setuid：少了 s/S/t/T 会与 ls -l 输出对不上
        assert_eq!(
            render_mode_text(FileKind::File, 0o4755 & PERMISSION_BITS),
            "-rwsr-xr-x"
        );
        // setgid 但组无执行位 → 大写 S（占组的 x 位），不是少一位
        assert_eq!(
            render_mode_text(FileKind::File, 0o2644 & PERMISSION_BITS),
            "-rw-r-Sr--"
        );
        assert_eq!(
            render_mode_text(FileKind::File, 0o2755 & PERMISSION_BITS),
            "-rwxr-sr-x"
        );
        assert_eq!(
            render_mode_text(FileKind::Dir, 0o1777 & PERMISSION_BITS),
            "drwxrwxrwt"
        );
        assert_eq!(
            render_mode_text(FileKind::Dir, 0o1644 & PERMISSION_BITS),
            "drw-r--r-T"
        );
    }

    #[test]
    fn kind_from_mode_covers_every_file_type() {
        assert_eq!(FileKind::from_mode(0o040000 | 0o755), FileKind::Dir);
        assert_eq!(FileKind::from_mode(0o100000 | 0o644), FileKind::File);
        assert_eq!(FileKind::from_mode(0o120000 | 0o777), FileKind::Symlink);
        assert_eq!(FileKind::from_mode(0o140000 | 0o755), FileKind::Socket);
        assert_eq!(FileKind::from_mode(0o010000 | 0o600), FileKind::Fifo);
        assert_eq!(FileKind::from_mode(0o060000 | 0o660), FileKind::Block);
        assert_eq!(FileKind::from_mode(0o020000 | 0o666), FileKind::Char);
        // 未知类型位不得猜成普通文件
        assert_eq!(FileKind::from_mode(0o170000), FileKind::Other);
        assert_eq!(kind_name(FileKind::Other), "other");
    }
}
