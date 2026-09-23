//! 文件页的写侧能力（增 / 删 / 改），由 `FilesystemProvider` 分发进来。
//!
//! 为什么不走桌面端 `adb shell rm/mv/chmod`：那是把路径拼进一条 shell 文本里执行,
//! 返回值只说明"命令跑完了"，不说明"东西真的被删了/改名了"；而且拼串一旦遇到空格、
//! 引号、`;` 就是行为变形。这里全部用 `std::fs` 直接操作参数数组，**执行完再读回一次**
//! 复核（与 D039 对 SO 替换的口径一致：结论来自设备自证，不来自退出码）。
//!
//! 与读侧最重要的一条区别是**范围**：读可以浏览全盘（能不能读由内核 DAC/SELinux 决定，
//! 我们不去猜），写不可以。写操作默认只允许落在 `/sdcard` 与 `/data/local/tmp`，
//! 越界一律 `permission_denied`；这两个根本身也永远不可删（`protected_root`）。
//! 需要放宽时用 `APP_REVERSE_TOOLS_AGENT_FS_WRITE_ROOTS=a,b` 显式配置。

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use agent_protocol::method::{
    FILESYSTEM_CHMOD, FILESYSTEM_MKDIR, FILESYSTEM_REMOVE, FILESYSTEM_RENAME,
};
use agent_protocol::{
    AgentError, ErrorCode, FilesystemChmodParams, FilesystemChmodResult, FilesystemMkdirParams,
    FilesystemMkdirResult, FilesystemRemoveParams, FilesystemRemoveResult, FilesystemRenameParams,
    FilesystemRenameResult, PERMISSION_BITS, render_mode_text,
};
use serde_json::Value;

use super::filesystem::{display, io_error, metadata_of, parse_params, serialize, split_last};
use agent_protocol::FileKind;

const WRITE_ROOTS_ENV: &str = "APP_REVERSE_TOOLS_AGENT_FS_WRITE_ROOTS";
const DEFAULT_WRITE_ROOTS: &str = "/sdcard,/data/local/tmp";
/// 递归删除前允许统计的条目上限：超过就拒绝，而不是"删了但说不清删了多少"。
const MAX_REMOVE_ENTRIES: usize = 20_000;

pub(crate) fn mkdir(params: Value) -> Result<Value, AgentError> {
    let params: FilesystemMkdirParams = parse_params(params)?;
    let target = resolve_for_write(&params.path)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&target) {
        let mode = metadata.mode() & PERMISSION_BITS;
        if metadata.is_dir() {
            // 已在：幂等返回，不改动设备（界面上要能看出"这次其实没建"）
            return serialize(FilesystemMkdirResult {
                path: display(&target),
                created: false,
                mode,
                mode_text: render_mode_text(FileKind::Dir, mode),
            });
        }
        return Err(bad_request(
            "同路径上已经有一个不是目录的东西",
            Some("path_exists_not_dir"),
            &target,
        ));
    }
    std::fs::create_dir(&target).map_err(|error| io_error("mkdir", &target, error))?;
    // 读回复核：目录真的在，才算创建成功
    let metadata =
        std::fs::symlink_metadata(&target).map_err(|error| io_error("stat", &target, error))?;
    if !metadata.is_dir() {
        return Err(internal(&format!(
            "mkdir 之后读回来不是目录: {}",
            display(&target)
        )));
    }
    let mode = metadata.mode() & PERMISSION_BITS;
    eprintln!(
        "audit method={FILESYSTEM_MKDIR} path={} created=true mode={:o}",
        display(&target),
        mode
    );
    serialize(FilesystemMkdirResult {
        path: display(&target),
        created: true,
        mode,
        // 目录是自己刚建的，类型确定；不要用掩掉文件类型位的 mode 去推 kind
        mode_text: render_mode_text(FileKind::Dir, mode),
    })
}

pub(crate) fn rename(params: Value) -> Result<Value, AgentError> {
    let params: FilesystemRenameParams = parse_params(params)?;
    let from = resolve_for_write(&params.from)?;
    // 源必须存在（lstat：符号链接本身也算存在，不跟随）
    let metadata = metadata_of(&from, false)?;
    let to = resolve_for_write(&params.to)?;
    if from == to {
        return serialize(FilesystemRenameResult {
            from: display(&from),
            to: display(&to),
            kind: FileKind::from_mode(metadata.mode()),
            mode: metadata.mode() & PERMISSION_BITS,
            mode_text: render_mode_text(
                FileKind::from_mode(metadata.mode()),
                metadata.mode() & PERMISSION_BITS,
            ),
            no_op: true,
        });
    }
    if std::fs::symlink_metadata(&to).is_ok() {
        // 目标已存在：拒绝，绝不静默覆盖（覆盖是另一个动作，界面要单独表达）
        return Err(bad_request(
            "目标位置已经有东西，不会被覆盖",
            Some("target_exists"),
            &to,
        ));
    }
    check_writable(&to)?;
    std::fs::rename(&from, &to).map_err(|error| io_error("rename", &from, error))?;
    // 读回复核：新位置在、旧位置没了
    let moved = std::fs::symlink_metadata(&to).map_err(|error| io_error("stat", &to, error))?;
    if std::fs::symlink_metadata(&from).is_ok() {
        return Err(internal(&format!(
            "rename 之后旧路径仍然存在: {}",
            display(&from)
        )));
    }
    let kind = FileKind::from_mode(moved.mode());
    let mode = moved.mode() & PERMISSION_BITS;
    eprintln!(
        "audit method={FILESYSTEM_RENAME} from={} to={} kind={:?}",
        display(&from),
        display(&to),
        kind
    );
    serialize(FilesystemRenameResult {
        from: display(&from),
        to: display(&to),
        kind,
        mode,
        mode_text: render_mode_text(kind, mode),
        no_op: false,
    })
}

pub(crate) fn remove(params: Value) -> Result<Value, AgentError> {
    let params: FilesystemRemoveParams = parse_params(params)?;
    let target = resolve_for_write(&params.path)?;
    let metadata = metadata_of(&target, false)?;
    let kind = FileKind::from_mode(metadata.mode());
    if is_protected_root(&target) {
        return Err(AgentError::new(
            ErrorCode::PermissionDenied,
            format!("这是允许的根目录本身，不能删除: {}", display(&target)),
        )
        .with_details(serde_json::json!({"reason": "protected_root", "path": display(&target)})));
    }
    if !params.recursive && matches!(kind, FileKind::Dir) {
        // 空目录可以直接删；非空目录必须显式递归，不替用户猜"里面那 300 个文件也一起删"
        let count = count_entries(&target)?;
        if count > 0 {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("目录非空（里面有 {count} 项），需要显式选择递归删除"),
            )
            .with_details(serde_json::json!({
                "reason": "directory_not_empty",
                "path": display(&target),
                "entries": count,
            })));
        }
    }
    let (freed, entries) = measure(&target)?;
    if entries > MAX_REMOVE_ENTRIES {
        return Err(bad_request(
            &format!("条目超过 {MAX_REMOVE_ENTRIES} 个，不敢递归删除"),
            Some("too_many_entries"),
            &target,
        ));
    }
    let result = if matches!(kind, FileKind::Dir) {
        if params.recursive {
            std::fs::remove_dir_all(&target)
        } else {
            std::fs::remove_dir(&target)
        }
    } else {
        std::fs::remove_file(&target)
    };
    result.map_err(|error| io_error("remove", &target, error))?;
    // 复核：再 lstat 一次，必须报"不存在"
    let gone = std::fs::symlink_metadata(&target).is_err();
    eprintln!(
        "audit method={FILESYSTEM_REMOVE} path={} removed={} kind={:?} recursive={} bytes={freed}",
        display(&target),
        gone,
        kind,
        params.recursive
    );
    if !gone {
        return Err(internal(&format!(
            "删除后仍能在设备上看到该路径: {}",
            display(&target)
        )));
    }
    serialize(FilesystemRemoveResult {
        path: display(&target),
        removed: true,
        was_dir: matches!(kind, FileKind::Dir),
        was_recursive: params.recursive,
        freed_bytes: freed,
    })
}

pub(crate) fn chmod(params: Value) -> Result<Value, AgentError> {
    let params: FilesystemChmodParams = parse_params(params)?;
    if params.mode & !0o777 != 0 {
        // 只接受 rwx 九位；setuid/setgid/sticky 不给（这些位能悄悄改变执行语义）
        return Err(bad_request(
            "权限位只支持 0o000-0o777，特殊位不在本能力范围内",
            Some("mode_bits_not_allowed"),
            Path::new(&params.path),
        ));
    }
    let target = resolve_for_write(&params.path)?;
    let previous = metadata_of(&target, false)?;
    let previous_mode = previous.mode() & PERMISSION_BITS;
    let previous_kind = FileKind::from_mode(previous.mode());
    // 符号链接的权限在内核上无意义（生效的是目标），这里明确拒，避免界面显示"改成功了"
    if matches!(previous_kind, FileKind::Symlink) {
        return Err(bad_request(
            "符号链接不支持改权限，请对它的目标操作",
            Some("chmod_on_symlink"),
            &target,
        ));
    }
    let mode = params.mode;
    let changed = previous_mode & 0o777 != mode;
    if changed {
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            .map_err(|error| io_error("chmod", &target, error))?;
    }
    // 读回复核：某些文件系统（FUSE 上的 /sdcard）会吃掉某些位，必须让它照实显示
    let after =
        std::fs::symlink_metadata(&target).map_err(|error| io_error("stat", &target, error))?;
    let actual = after.mode() & PERMISSION_BITS;
    let verified = actual & 0o777 == mode;
    eprintln!(
        "audit method={FILESYSTEM_CHMOD} path={} mode={:o} prev={:o} verified={verified}",
        display(&target),
        actual,
        previous_mode
    );
    serialize(FilesystemChmodResult {
        path: display(&target),
        mode: actual,
        mode_text: render_mode_text(after_kind(&after, previous_kind), actual),
        previous_mode,
        previous_mode_text: render_mode_text(previous_kind, previous_mode),
        verified,
    })
}

fn after_kind(metadata: &std::fs::Metadata, fallback: FileKind) -> FileKind {
    if metadata.is_dir() {
        FileKind::Dir
    } else {
        fallback
    }
}

/// 把请求路径解析成"可以安全判定的绝对路径"。两步，顺序有意义：
///
/// 1. **先按字面归一化并检查允许根**。真机上发现：只做第 2 步时，
///    `/sdcard/../sdcard2/x` 会先撞上"父目录不存在"（`not_found`），
///    用户看到的是一个说不清的理由——而它真正的问题是越界。字面检查先把这类
///    用 `..` 往外跳的请求用 `write_path_not_allowed` 拒掉。
/// 2. **再 canonicalize 父目录**（跟随符号链接），这样 `/sdcard/x` 里 `/sdcard` 指向
///    `/storage/emulated/0` 时，符号链接绕到白名单外的情况也拦得住。
///    最后一段原样接上——它可能还不存在（mkdir / rename 目标）。
fn resolve_for_write(raw: &str) -> Result<PathBuf, AgentError> {
    let requested = Path::new(raw);
    if requested.as_os_str().is_empty() || !requested.is_absolute() {
        return Err(bad_request(
            "必须是设备上的绝对路径",
            Some("path_not_absolute"),
            requested,
        ));
    }
    let lexical = normalize_lexically(requested);
    check_writable(&lexical)?;
    let (parent, last) = split_last(&lexical);
    let parent =
        std::fs::canonicalize(&parent).map_err(|error| io_error("canonicalize", &parent, error))?;
    // 范围检查落在**拼好的目标**上，不是父目录上：真机腿跑出来过一次误拒——
    // 对允许根本身做操作（例如删除 /data/local/tmp）时父目录是 /data/local，
    // 按父目录判范围会让本该由 protected_root 拒的请求先一步被拒成"越界"。
    let resolved = match last {
        Some(name) => parent.join(name),
        None => parent,
    };
    check_writable(&resolved)?;
    Ok(resolved)
}

/// 纯字面的路径归一化：消掉 `.` 与 `..`，不访问文件系统。
/// `..` 越过根时停在根（`/../../etc` -> `/etc`），不产生相对路径。
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(name) => parts.push(name.to_os_string()),
            // 前面已经要求过绝对路径，这里不会出现 Prefix
            std::path::Component::Prefix(_) => {}
        }
    }
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    out
}

fn write_roots() -> Vec<PathBuf> {
    parse_write_roots(
        &std::env::var(WRITE_ROOTS_ENV).unwrap_or_else(|_| DEFAULT_WRITE_ROOTS.to_owned()),
    )
    .into_iter()
    .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
    .collect()
}

/// 解析允许根配置。**任何解析不出东西的情况都退回默认两根**：
/// 环境变量被手滑设成空串/全是相对路径时，最危险的失败方式是"什么都不拦"，
/// 所以这里宁可少给权限也不放开全盘。
fn parse_write_roots(raw: &str) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = raw
        .split(',')
        .map(str::trim)
        .filter(|value| value.starts_with('/') && value.len() > 1)
        .map(PathBuf::from)
        .collect();
    if roots.is_empty() {
        return DEFAULT_WRITE_ROOTS.split(',').map(PathBuf::from).collect();
    }
    roots
}

fn check_writable(path: &Path) -> Result<(), AgentError> {
    // 两种写法都要认：`/sdcard/Download`（配置里的字面根）与
    // `/storage/emulated/0/Download`（canonical 之后的根）。只认后者会让界面上
    // 用 `/sdcard/...` 的合法请求全部被误拒。
    let raw = std::env::var(WRITE_ROOTS_ENV).unwrap_or_else(|_| DEFAULT_WRITE_ROOTS.to_owned());
    let configured = parse_write_roots(&raw);
    let accepted = configured.iter().any(|root| path.starts_with(root))
        || write_roots().iter().any(|root| path.starts_with(root));
    if accepted {
        return Ok(());
    }
    Err(AgentError::new(
        ErrorCode::PermissionDenied,
        format!("写操作不在允许的目录范围内: {}", path.display()),
    )
    .with_details(serde_json::json!({
        "reason": "write_path_not_allowed",
        "path": display(path),
        "allowed_write_roots": configured
            .iter()
            .map(|root| display(root))
            .collect::<Vec<_>>(),
    })))
}

/// 允许的根本身永远不可删：`/data/local/tmp` 被删掉会让 Agent 自己的暂存与日志
/// 无处可放，`/sdcard` 其实是符号链接（真机上第一版判定就漏了它，因为它比较的是
/// 解析后的路径），所以两侧都先 canonical 再比。
fn is_protected_root(path: &Path) -> bool {
    // 比较用**解析后**的路径：`/sdcard` 是符号链接，只有 canonical 之后才认得出它就是那个根
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    write_roots().contains(&canonical)
}

fn count_entries(path: &Path) -> Result<usize, AgentError> {
    let mut count = 0usize;
    let entries = std::fs::read_dir(path).map_err(|error| io_error("read_dir", path, error))?;
    for entry in entries.take(MAX_REMOVE_ENTRIES + 1) {
        let _ = entry.map_err(|error| io_error("read_dir", path, error))?;
        count += 1;
    }
    Ok(count)
}

/// 删除前把字节数算清楚（也兼作条目上限检查）。算不清就不删。
fn measure(path: &Path) -> Result<(u64, usize), AgentError> {
    let metadata = metadata_of(path, false)?;
    if !metadata.is_dir() {
        return Ok((metadata.len(), 1));
    }
    let mut bytes = metadata.len();
    let mut entries = 1usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).map_err(|error| io_error("read_dir", &dir, error))?;
        for item in read {
            let item = item.map_err(|error| io_error("read_dir", &dir, error))?;
            entries += 1;
            if entries > MAX_REMOVE_ENTRIES {
                return Ok((bytes, entries));
            }
            let child = item.path();
            // 目录内条目按不跟随的方式统计；符号链接的正文大小按链接自身算（与 du 一致）
            let meta = metadata_of(&child, false)?;
            bytes = bytes.saturating_add(meta.len());
            if meta.is_dir() {
                stack.push(child);
            }
        }
    }
    Ok((bytes, entries))
}

fn bad_request(message: &str, reason: Option<&str>, path: &Path) -> AgentError {
    let mut error = AgentError::new(ErrorCode::InvalidRequest, message.to_owned());
    if let Some(reason) = reason {
        error = error.with_details(serde_json::json!({
            "reason": reason,
            "path": display(path),
        }));
    }
    error
}

fn internal(message: &str) -> AgentError {
    AgentError::new(ErrorCode::Internal, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写侧的允许根只能显式配置放宽，且**默认非空**——环境变量被设成空串时
    /// 必须退回默认两根，而不是变成"全盘可写"。
    #[test]
    fn write_roots_never_widen_to_everything() {
        // 空配置 / 全是相对路径 -> 回到默认两根，绝不等于"全盘可写"
        for raw in ["", "   ", "relative", "a/b,,"] {
            let roots = parse_write_roots(raw);
            assert!(
                roots.contains(&PathBuf::from("/sdcard"))
                    && roots.contains(&PathBuf::from("/data/local/tmp")),
                "{raw:?} 应退回默认两根，实际 {roots:?}"
            );
        }
        // 显式配置时只认绝对路径，且不会偷偷加回默认
        assert_eq!(
            parse_write_roots("/data/local/tmp"),
            vec![PathBuf::from("/data/local/tmp")]
        );
        assert_eq!(
            parse_write_roots("relative,,/ok"),
            vec![PathBuf::from("/ok")]
        );
    }

    /// 越界路径必须在**任何 fs 调用之前**被拒（这条不依赖设备，本机就能验）。
    #[test]
    fn check_writable_rejects_outside_roots() {
        let case = |path: &Path, roots: &[&str]| -> bool {
            let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
            if roots.iter().any(|root| path.starts_with(root)) {
                return true;
            }
            false
        };
        assert!(case(Path::new("/data/local/tmp/x"), &["/data/local/tmp"]));
        assert!(case(Path::new("/sdcard/Download/a.txt"), &["/sdcard"]));
        assert!(!case(
            Path::new("/data/data/com.example/x"),
            &["/sdcard", "/data/local/tmp"]
        ));
        assert!(!case(
            Path::new("/system/app/x"),
            &["/sdcard", "/data/local/tmp"]
        ));
        // 前缀相似但不是子路径的必须拒（`/sdcard2` 不算 `/sdcard` 里面）
        assert!(!case(Path::new("/sdcard2/x"), &["/sdcard"]));
    }

    /// 特殊位（setuid/setgid/sticky）必须在**碰到文件系统之前**被拒：
    /// 这些位会悄悄改变执行语义，不属于"改权限"这个能力的范围。
    /// 断言走真实的 chmod 入口，而不是对着常量比划。
    #[test]
    fn chmod_rejects_special_bits_before_touching_the_disk() {
        for mode in [0o4755_u32, 0o2755, 0o1777, 0o10777] {
            let error = chmod(serde_json::json!({
                "path": "/data/local/tmp/does-not-matter-here",
                "mode": mode,
            }))
            .expect_err("特殊位必须被拒");
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{mode:o}: {error:?}");
            assert_eq!(
                error.details.expect("要带结构化理由")["reason"],
                serde_json::json!("mode_bits_not_allowed")
            );
        }
        // 普通三位权限不因为位检查被误拒（它会在后面的路径检查里失败，那是另一回事）
        let error = chmod(serde_json::json!({
            "path": "/definitely/not/here/ar74-unit-test",
            "mode": 0o600,
        }))
        .expect_err("不存在的路径应当失败");
        assert_ne!(
            error
                .details
                .as_ref()
                .and_then(|d| d.get("reason"))
                .and_then(serde_json::Value::as_str),
            Some("mode_bits_not_allowed"),
            "位检查不能挡住合法请求: {error:?}"
        );
    }
}
