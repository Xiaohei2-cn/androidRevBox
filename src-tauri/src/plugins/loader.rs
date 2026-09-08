//! 动态库加载器（C ABI v1）：发现受控目录 → 校验 manifest → libloading 打开 →
//! 符号检查 → abi 门禁 → init。调用/释放/关停经 LoadedPlugin 封装。
//! 契约要点（plugin_api.h）：
//! - 输出由插件分配、仅经插件自己的 at_plugin_free 归还（用 Box<[u8]> 协议对齐 SDK）；
//! - 所有调用串行化（LOADED_CALL_LOCK），插件实现无需自带锁；
//! - 任何加载失败返回可读错误，禁止 panic。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use libloading::{Library, Symbol};

use crate::plugins::manifest::{PluginManifest, current_platform_key};

/// 加载期错误（展示文案直接面向用户）
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("读取 manifest 失败: {0}")]
    ManifestIo(String),
    #[error(transparent)]
    Manifest(#[from] crate::plugins::manifest::ManifestError),
    #[error("打开动态库失败: {0}")]
    Open(String),
    #[error("缺少导出符号: {0}")]
    MissingSymbol(String),
    #[error("at_plugin_init 失败，插件返回错误码 {0}")]
    InitFailed(i32),
    #[error("abi 符号版本非法: {0}")]
    BadAbi(u32),
}

/// 单次调用的资源上限（P6）。max_input/max_output 为 0 表示不限制。
#[derive(Debug, Clone, Copy)]
pub struct CallLimits {
    pub timeout: std::time::Duration,
    pub max_input: usize,
    pub max_output: usize,
}

impl Default for CallLimits {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(30),
            max_input: 0,
            max_output: 0,
        }
    }
}

/// 带限制调用（call_checked）的错误。Timeout 后插件状态未知，
/// 调用方必须把插件置为故障态（禁止继续调用，直到重新加载）。
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("插件调用超时（{0:?}），已停止该插件")]
    Timeout(std::time::Duration),
    #[error("输入超过上限 {max} 字节")]
    InputTooLarge { max: usize },
    #[error("插件输出超过上限 {max} 字节，已丢弃")]
    OutputTooLarge { max: usize },
    #[error("插件调用线程异常退出")]
    WorkerGone,
}

/// at_plugin_info 返回的 C 结构（与 plugin_api.h 布局一致）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AtPluginInfoC {
    pub abi_version: u32,
    pub id: *const std::ffi::c_char,
    pub name: *const std::ffi::c_char,
    pub version: *const std::ffi::c_char,
    pub plugin_type: *const std::ffi::c_char,
}

unsafe impl Send for AtPluginInfoC {}

/// 所有对「任意插件动态库导出函数」的调用必须持有该锁：
/// 插件库可能与主程序共享分配器，跨 dylib 的并发 malloc/free 在部分平台不安全。
static LOADED_CALL_LOCK: Mutex<()> = Mutex::new(());

/// 在全局插件调用锁内执行 f（P6 安装/启停的卸载段必须用它：
/// 防止「另一个线程正在插件代码内执行时 dlclose」的 UB）。
pub fn with_call_lock<R>(f: impl FnOnce() -> R) -> R {
    let _g = LOADED_CALL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    f()
}

/// 候选插件目录清单：(目录名, canonical 路径) 与预检错误（目录名, 错误）
pub type CandidateDirs = (Vec<(String, PathBuf)>, Vec<(String, LoadError)>);

/// 枚举 root 下可尝试加载的插件目录：跳过文件与点开头的管理目录
/// （`.backups`/`.staging`），符号链接逃逸的目录进 errors。
/// discover_and_load 与 PluginService::scan 共用。
pub fn list_candidate_dirs(root: &Path) -> CandidateDirs {
    let mut dirs = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        // 目录不存在 = 还没有插件，正常
        return (dirs, errors);
    };
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        // 受控目录防逃逸：插件目录（符号链接解析后）必须在 root 之下
        match dir.canonicalize() {
            Ok(d) if d.starts_with(&canonical_root) => dirs.push((name, d)),
            _ => errors.push((
                name,
                LoadError::Manifest(crate::plugins::manifest::ManifestError::PathEscape),
            )),
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    (dirs, errors)
}

/// 已加载插件（持有 Library；drop 即 shutdown + close）
pub struct LoadedPlugin {
    _lib: Library,
    info: AtPluginInfoC,
    manifest: PluginManifest,
    dir: PathBuf,
    shutdown: extern "C" fn(),
    call: unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32,
    free_out: unsafe extern "C" fn(*mut u8, usize),
}

// 调用已全部经 LOADED_CALL_LOCK 串行化；Library/Symbol 本身不被跨线程触碰
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // shutdown 是安全 extern fn（SDK 宏内部已 catch_unwind）；仍持锁防与其它 call 竞争
        let _g = LOADED_CALL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        (self.shutdown)();
    }
}

impl LoadedPlugin {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 实际 ABI id/name（来自动态库本身，与 manifest 交叉验证用）
    pub fn abi_info(&self) -> AbiInfo {
        AbiInfo {
            abi_version: self.info.abi_version,
            id: cstr_to_string(self.info.id),
            name: cstr_to_string(self.info.name),
            version: cstr_to_string(self.info.version),
            plugin_type: cstr_to_string(self.info.plugin_type),
        }
    }

    /// 调用插件：返回 (C 错误码, 输出)。输出在本函数内已被复制为 Vec 并归还插件内存。
    pub fn call(&self, input: &[u8]) -> (i32, Vec<u8>) {
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let _g = LOADED_CALL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let code = unsafe { (self.call)(input.as_ptr(), input.len(), &mut out_ptr, &mut out_len) };
        if out_ptr.is_null() || out_len == 0 {
            return (code, Vec::new());
        }
        // 复制输出后立即经插件的 free 归还（所有权：插件分配 → 插件释放）
        let slice = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
        let data = slice.to_vec();
        unsafe { (self.free_out)(out_ptr, out_len) };
        (code, data)
    }

    /// 带资源限制的调用（P6）。工作线程内执行真实 call（同样持全局锁），
    /// 超时后立即返回错误——注意工作线程此刻可能仍阻塞在插件内部，
    /// in-process 无法抢占，后续调用会等它真正返回；调用方必须置故障态，
    /// 且【禁止卸载】该插件（Library 可能在被执行中，dlclose 是 UB）——
    /// 服务层把 Arc 移入 quarantine 保活至进程退出。
    pub fn call_checked(
        &self,
        input: &[u8],
        limits: &CallLimits,
    ) -> Result<(i32, Vec<u8>), CallError> {
        if limits.max_input > 0 && input.len() > limits.max_input {
            return Err(CallError::InputTooLarge {
                max: limits.max_input,
            });
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let input = input.to_vec();
        // fn 指针是 Copy；Library 的存活由调用方的 quarantine 约定保证
        let call_fn = self.call;
        let free_fn = self.free_out;
        std::thread::Builder::new()
            .name("plugin-call".into())
            .spawn(move || {
                let mut out_ptr: *mut u8 = std::ptr::null_mut();
                let mut out_len: usize = 0;
                let _g = LOADED_CALL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
                let code =
                    unsafe { (call_fn)(input.as_ptr(), input.len(), &mut out_ptr, &mut out_len) };
                let data = if out_ptr.is_null() || out_len == 0 {
                    Vec::new()
                } else {
                    let slice = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
                    let d = slice.to_vec();
                    unsafe { (free_fn)(out_ptr, out_len) };
                    d
                };
                let _ = tx.send((code, data));
            })
            .map_err(|_| CallError::WorkerGone)?;
        match rx.recv_timeout(limits.timeout) {
            Ok((code, data)) => {
                if limits.max_output > 0 && data.len() > limits.max_output {
                    return Err(CallError::OutputTooLarge {
                        max: limits.max_output,
                    });
                }
                Ok((code, data))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err(CallError::Timeout(limits.timeout))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(CallError::WorkerGone),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiInfo {
    pub abi_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
}

fn cstr_to_string(p: *const std::ffi::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// 发现 + 校验 + 加载插件目录下所有合法插件。
/// 单个插件失败不阻断整体扫描（记录进 errors 供 UI 展示）。
/// 点开头的目录（`.backups`/`.staging` 等管理目录，P6 起）一律跳过。
pub fn discover_and_load(root: &Path) -> (HashMap<String, LoadedPlugin>, Vec<(String, LoadError)>) {
    let (candidates, mut errors) = list_candidate_dirs(root);
    let mut loaded = HashMap::new();
    for (name, dir) in candidates {
        match load_one(&dir) {
            Ok(plugin) => {
                loaded.insert(plugin.manifest().id.clone(), plugin);
            }
            Err(e) => errors.push((name, e)),
        }
    }
    (loaded, errors)
}

/// 加载单个插件目录（发现/手动重试共用）
pub fn load_one(dir: &Path) -> Result<LoadedPlugin, LoadError> {
    let manifest_text = std::fs::read_to_string(dir.join("manifest.json"))
        .map_err(|e| LoadError::ManifestIo(e.to_string()))?;
    let manifest = PluginManifest::parse(&manifest_text).map_err(LoadError::ManifestIo)?;
    manifest.validate(&current_platform_key())?;

    let rel = manifest
        .entry_for(&current_platform_key())
        .expect("validate 已保证存在");
    let lib_path = safe_join(dir, rel)?;
    let lib = unsafe { Library::new(&lib_path) }
        .map_err(|e| LoadError::Open(format!("{}: {e}", lib_path.display())))?;

    // 符号提取收进作用域：拷成裸 fn 指针（Copy，不再借用 lib），块结束即释放 lib 借用
    let (info, init_fn, call_fn, free_fn, shutdown_fn) = {
        let info_sym: Symbol<extern "C" fn() -> *const AtPluginInfoC> = unsafe {
            lib.get(b"at_plugin_info")
                .map_err(|_| LoadError::MissingSymbol("at_plugin_info".into()))?
        };
        let init_sym: Symbol<unsafe extern "C" fn(*const std::ffi::c_void) -> i32> = unsafe {
            lib.get(b"at_plugin_init")
                .map_err(|_| LoadError::MissingSymbol("at_plugin_init".into()))?
        };
        let call_sym: Symbol<
            unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32,
        > = unsafe {
            lib.get(b"at_plugin_call")
                .map_err(|_| LoadError::MissingSymbol("at_plugin_call".into()))?
        };
        let free_sym: Symbol<unsafe extern "C" fn(*mut u8, usize)> = unsafe {
            lib.get(b"at_plugin_free")
                .map_err(|_| LoadError::MissingSymbol("at_plugin_free".into()))?
        };
        let shutdown_sym: Symbol<extern "C" fn()> = unsafe {
            lib.get(b"at_plugin_shutdown")
                .map_err(|_| LoadError::MissingSymbol("at_plugin_shutdown".into()))?
        };

        let info_ptr = info_sym();
        if info_ptr.is_null() {
            return Err(LoadError::MissingSymbol("at_plugin_info 返回 NULL".into()));
        }
        let info = unsafe { *info_ptr };
        (info, *init_sym, *call_sym, *free_sym, *shutdown_sym)
    };

    if info.abi_version != crate::plugins::manifest::HOST_ABI_VERSION {
        return Err(LoadError::BadAbi(info.abi_version));
    }
    // 动态库自报 id 必须与 manifest 一致（防目录/声明错位）
    if cstr_to_string(info.id) != manifest.id {
        return Err(LoadError::Manifest(
            crate::plugins::manifest::ManifestError::BadId(format!(
                "manifest={} 实际={}",
                manifest.id,
                cstr_to_string(info.id)
            )),
        ));
    }

    // init（v1 host 指针恒 NULL）；持全局锁（init 可能分配资源）
    let rc = {
        let _g = LOADED_CALL_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { (init_fn)(std::ptr::null()) }
    };
    if rc != 0 {
        return Err(LoadError::InitFailed(rc));
    }

    Ok(LoadedPlugin {
        _lib: lib,
        info,
        manifest,
        dir: dir.to_path_buf(),
        shutdown: shutdown_fn,
        call: call_fn,
        free_out: free_fn,
    })
}

/// manifest.entry 相对路径拼接（拒绝绝对与 ..，防目录逃逸）
pub fn safe_join(dir: &Path, rel: &str) -> Result<PathBuf, LoadError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(LoadError::Manifest(
            crate::plugins::manifest::ManifestError::PathEscape,
        ));
    }
    Ok(dir.join(rel_path))
}

/// 只解析 + 校验 manifest，不加载动态库（P6：禁用插件注册进列表用）。
/// 同时确认当前平台产物文件存在。
pub fn inspect(dir: &Path) -> Result<PluginManifest, LoadError> {
    let manifest_text = std::fs::read_to_string(dir.join("manifest.json"))
        .map_err(|e| LoadError::ManifestIo(e.to_string()))?;
    let manifest = PluginManifest::parse(&manifest_text).map_err(LoadError::ManifestIo)?;
    manifest.validate(&current_platform_key())?;
    let rel = manifest
        .entry_for(&current_platform_key())
        .expect("validate 已保证存在");
    let artifact = safe_join(dir, rel)?;
    if !artifact.is_file() {
        return Err(LoadError::Open(format!(
            "平台产物不存在: {}",
            artifact.display()
        )));
    }
    Ok(manifest)
}
