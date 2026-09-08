//! PluginService（P6）：插件从「能加载」到「可管理」。
//! - 生命周期：scan / install(含升级) / rollback / uninstall / set_enabled
//! - 调用分发：in-process（C ABI）与 process（stdio JSON-RPC）两种传输
//! - 资源限制：单次调用超时 + 输入/输出载荷上限（键 app.plugins.*，ConfigService 校验）
//! - 事件：生命周期变化经 PluginEventSink 推 `plugin://changed`
//!
//! 已知局限（文档化，总案 §11/PHASES §9.3）：
//! in-process 插件调用超时后无法安全卸载（Library 可能在执行中，dlclose 是 UB），
//! 句柄移入 quarantine 保活至进程退出，该插件置为禁用；真隔离用 process 插件。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tauri::Emitter;

use crate::core::error::{CoreError, CoreResult};
use crate::core::ipc::{AppEvent, event_names};
use crate::db::Db;
use crate::plugins::install;
use crate::plugins::loader::{self, AbiInfo, CallError, CallLimits, LoadedPlugin};
use crate::plugins::manifest::{PluginManifest, current_platform_key};
use crate::plugins::plugin_repo::{self, PluginRow};
use crate::plugins::process::{ProcessError, ProcessInfo, ProcessPlugin};
use crate::services::config_service::{
    ConfigService, KEY_PLUGIN_CALL_TIMEOUT_MS, KEY_PLUGIN_MAX_PAYLOAD_KB,
};

pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_PAYLOAD_KB: u64 = 8_192;

/// 生命周期事件出口（§1.5 纪律：可测逻辑抽象成 trait）
pub trait PluginEventSink: Send + Sync {
    fn plugin_changed(&self, plugin_id: &str, phase: &str, version: Option<&str>);
}

/// 真实实现：经 Tauri AppHandle emit `plugin://changed`
pub struct TauriPluginEventSink(pub tauri::AppHandle);

impl PluginEventSink for TauriPluginEventSink {
    fn plugin_changed(&self, plugin_id: &str, phase: &str, version: Option<&str>) {
        let evt = AppEvent::new(
            event_names::PLUGIN_CHANGED,
            json!({ "pluginId": plugin_id, "phase": phase, "version": version }),
        );
        if let Err(e) = self.0.emit(evt.event, &evt) {
            tracing::warn!(error = %e, "plugin://changed 事件发送失败");
        }
    }
}

/// 已加载句柄：两种传输
pub enum LoadedKind {
    InProcess(Arc<LoadedPlugin>),
    /// process 插件 + 握手后缓存的 ABI 信息（list 不触发 spawn）
    Process(Arc<ProcessPlugin>, ProcessInfo),
}

impl LoadedKind {
    fn manifest(&self) -> &PluginManifest {
        match self {
            LoadedKind::InProcess(p) => p.manifest(),
            LoadedKind::Process(p, _) => p.manifest(),
        }
    }

    fn abi_info(&self) -> AbiInfo {
        match self {
            LoadedKind::InProcess(p) => p.abi_info(),
            LoadedKind::Process(_, info) => AbiInfo {
                abi_version: 1,
                id: info.id.clone(),
                name: info.name.clone(),
                version: info.version.clone(),
                plugin_type: info.plugin_type.clone(),
            },
        }
    }
}

pub struct PluginService {
    db: Arc<Db>,
    config: Arc<ConfigService>,
    root: PathBuf,
    loaded: Mutex<HashMap<String, LoadedKind>>,
    /// 超时插件的句柄收容所：绝不卸载（dlclose UB 防线），进程退出才释放
    quarantined: Mutex<Vec<Arc<LoadedPlugin>>>,
    /// 最近一次故障/错误（UI 展示；重启即清）
    last_error: Mutex<HashMap<String, String>>,
    events: Arc<dyn PluginEventSink>,
}

impl PluginService {
    pub fn new(
        db: Arc<Db>,
        config: Arc<ConfigService>,
        root: PathBuf,
        events: Arc<dyn PluginEventSink>,
    ) -> Self {
        Self {
            db,
            config,
            root,
            loaded: Mutex::new(HashMap::new()),
            quarantined: Mutex::new(Vec::new()),
            last_error: Mutex::new(HashMap::new()),
            events,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn emit(&self, id: &str, phase: &str, version: Option<&str>) {
        self.events.plugin_changed(id, phase, version);
    }

    fn set_last_error(&self, id: &str, msg: Option<String>) {
        let mut guard = self.last_error.lock().unwrap_or_else(|p| p.into_inner());
        match msg {
            Some(m) => {
                guard.insert(id.to_string(), m);
            }
            None => {
                guard.remove(id);
            }
        }
    }

    /// 调用资源限制（从配置读，越界/缺失用默认）
    fn call_limits(&self) -> (CallLimits, Duration) {
        let timeout_ms = self
            .config
            .get(
                KEY_PLUGIN_CALL_TIMEOUT_MS,
                &DEFAULT_CALL_TIMEOUT_MS.to_string(),
            )
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| (100..=600_000).contains(n))
            .unwrap_or(DEFAULT_CALL_TIMEOUT_MS);
        let payload_kb = self
            .config
            .get(
                KEY_PLUGIN_MAX_PAYLOAD_KB,
                &DEFAULT_MAX_PAYLOAD_KB.to_string(),
            )
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| (1..=65_536).contains(n))
            .unwrap_or(DEFAULT_MAX_PAYLOAD_KB);
        let max_bytes = (payload_kb * 1024) as usize;
        (
            CallLimits {
                timeout: Duration::from_millis(timeout_ms),
                max_input: max_bytes,
                max_output: max_bytes,
            },
            Duration::from_millis(timeout_ms),
        )
    }

    /// 按目录加载（transport 分发）；失败记录 last_error
    fn load_dir(&self, dir: &Path) -> Result<(String, LoadedKind), String> {
        let manifest = loader::inspect(dir).map_err(|e| e.to_string())?;
        let platform = current_platform_key();
        let rel = manifest
            .entry_for(&platform)
            .ok_or_else(|| format!("manifest 缺少当前平台产物: {platform}"))?;
        if manifest.is_process() {
            let exe = loader::safe_join(dir, rel).map_err(|e| e.to_string())?;
            let pp = ProcessPlugin::new(manifest.clone(), exe);
            let info = pp.info().map_err(|e| e.to_string())?;
            Ok((manifest.id.clone(), LoadedKind::Process(Arc::new(pp), info)))
        } else {
            let lp = loader::load_one(dir).map_err(|e| e.to_string())?;
            Ok((manifest.id.clone(), LoadedKind::InProcess(Arc::new(lp))))
        }
    }

    /// 扫描受控目录：合法插件注册入库（enabled 保留用户选择）；
    /// 被禁用的插件只注册不加载；加载失败进 errors 不阻断整体。
    pub fn scan(&self) -> CoreResult<ScanReport> {
        let (candidates, candidate_errors) = loader::list_candidate_dirs(&self.root);
        let mut errors: Vec<(String, String)> = candidate_errors
            .into_iter()
            .map(|(d, e)| (d, e.to_string()))
            .collect();
        let mut loaded_new: HashMap<String, LoadedKind> = HashMap::new();
        let mut ok_ids = Vec::new();

        for (name, dir) in candidates {
            let dir_str = dir.to_string_lossy().into_owned();
            // manifest 先解析：禁用插件不加载也要注册行
            let manifest = match loader::inspect(&dir) {
                Ok(m) => m,
                Err(e) => {
                    errors.push((name, e.to_string()));
                    continue;
                }
            };
            let id = manifest.id.clone();
            let enabled = self.db_enabled(&id).unwrap_or(true);
            let row = PluginRow {
                id: id.clone(),
                version: manifest.version.clone(),
                plugin_type: manifest.plugin_type.clone(),
                path: dir_str,
                enabled,
                abi: manifest.abi,
                transport: manifest.transport().to_string(),
            };
            plugin_repo::upsert(&self.db, &row)?;
            ok_ids.push(id.clone());
            if !enabled {
                continue; // 禁用：注册不加载
            }
            match self.load_dir(&dir) {
                Ok((_, kind)) => {
                    loaded_new.insert(id, kind);
                }
                Err(e) => {
                    self.set_last_error(&id, Some(e.clone()));
                    errors.push((name, e));
                }
            }
        }
        // 目录里消失的插件：清库、移出内存（drop → shutdown）
        {
            let mut guard = self.loaded.lock().expect("plugin map lock");
            loader::with_call_lock(|| {
                guard.retain(|id, _| ok_ids.contains(id));
            });
        }
        *self.loaded.lock().expect("plugin map lock") = loaded_new;
        plugin_repo::remove_except(&self.db, &ok_ids)?;

        let errors = errors
            .into_iter()
            .map(|(dir, e)| PluginError { dir, error: e })
            .collect();
        Ok(ScanReport {
            loaded: self.loaded_ids(),
            errors,
        })
    }

    fn loaded_ids(&self) -> Vec<String> {
        self.loaded
            .lock()
            .expect("plugin map lock")
            .keys()
            .cloned()
            .collect()
    }

    fn db_enabled(&self, id: &str) -> Option<bool> {
        plugin_repo::get(&self.db, id)
            .ok()
            .flatten()
            .map(|r| r.enabled)
    }

    /// 插件列表（= 详情源：含 ABI 自报信息、传输方式、回滚可用性、最近错误）
    pub fn list(&self) -> CoreResult<Vec<PluginView>> {
        let guard = self.loaded.lock().expect("plugin map lock");
        let rows = plugin_repo::list(&self.db)?;
        let last_errors = self.last_error.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::new();
        for r in rows {
            let kind = guard.get(&r.id);
            let rollback_available = install::latest_backup(&self.root, &r.id).is_some();
            out.push(PluginView {
                id: r.id.clone(),
                name: kind.map(|k| k.manifest().name.clone()).unwrap_or_default(),
                version: r.version,
                plugin_type: r.plugin_type,
                abi: r.abi,
                transport: r.transport,
                enabled: r.enabled,
                loaded: kind.is_some(),
                dir: Some(r.path),
                capabilities: kind
                    .map(|k| k.manifest().capabilities.clone())
                    .unwrap_or_default(),
                abi_info: kind.map(|k| AbiInfoView::from(&k.abi_info())),
                rollback_available,
                last_error: last_errors.get(&r.id).cloned(),
            });
        }
        Ok(out)
    }

    pub fn manifest_of(&self, id: &str) -> Option<PluginManifest> {
        self.loaded
            .lock()
            .expect("plugin map lock")
            .get(id)
            .map(|k| k.manifest().clone())
    }

    /// 调用插件（transport 分发 + 资源限制）。禁用/未加载返回可读错误。
    pub fn call(&self, id: &str, input: &[u8]) -> CoreResult<(i32, Vec<u8>)> {
        if let Some(row) = plugin_repo::get(&self.db, id)? {
            if !row.enabled {
                return Err(CoreError::Internal(format!(
                    "插件已禁用: {id}（在插件中心启用后再调用）"
                )));
            }
        }
        let kind = {
            let guard = self.loaded.lock().expect("plugin map lock");
            // LoadedKind 不实现 Clone——按变体提取 Arc
            match guard.get(id) {
                Some(LoadedKind::InProcess(p)) => Some(LoadedKind::InProcess(p.clone())),
                Some(LoadedKind::Process(p, info)) => {
                    Some(LoadedKind::Process(p.clone(), info.clone()))
                }
                None => None,
            }
        };
        let (limits, timeout) = self.call_limits();
        match kind {
            Some(LoadedKind::InProcess(p)) => match p.call_checked(input, &limits) {
                Ok(pair) => Ok(pair),
                Err(CallError::Timeout(t)) => {
                    // 故障处理：句柄进 quarantine（禁卸载）、自动禁用、记录错误
                    self.quarantined.lock().expect("quarantine").push(p);
                    loader::with_call_lock(|| {
                        self.loaded.lock().expect("plugin map lock").remove(id);
                    });
                    let _ = plugin_repo::set_enabled(&self.db, id, false);
                    self.set_last_error(
                        id,
                        Some(format!(
                            "调用超时（{t:?}），已停止并禁用该插件；如需恢复请重新启用"
                        )),
                    );
                    self.emit(id, "faulted", None);
                    Err(CoreError::Internal(format!(
                        "插件 {id} 调用超时（{t:?}），已停止并禁用；可尝试重新启用"
                    )))
                }
                Err(e) => Err(CoreError::Internal(e.to_string())),
            },
            Some(LoadedKind::Process(p, _)) => match p.call(input, timeout) {
                Ok(pair) => Ok(pair),
                // 崩溃/超时：进程已回收，下次调用懒重启；主程序不受影响
                Err(e @ (ProcessError::Crashed { .. } | ProcessError::Timeout(_))) => {
                    self.set_last_error(id, Some(e.to_string()));
                    self.emit(id, "faulted", None);
                    Err(CoreError::Internal(format!("进程插件 {id}: {e}")))
                }
                Err(e) => Err(CoreError::Internal(e.to_string())),
            },
            None => Err(CoreError::Internal(format!(
                "插件未加载或不存在: {id}（可能被禁用或上次加载失败，请刷新扫描）"
            ))),
        }
    }

    /// 安装 / 升级（同一入口，按 id 是否已存在自动判定）。
    /// 流程：校验源 → 版本门槛 → 卸旧句柄 → staging → 原子替换 → 加载验证 →
    /// 失败自动回滚（恢复旧版），成功保留最新备份供回滚。
    pub fn install(&self, src: &Path) -> CoreResult<PluginView> {
        let platform = current_platform_key();
        let manifest = install::validate_source(src, &platform)?;
        let id = manifest.id.clone();

        let target = install::target_dir(&self.root, &id);
        let existed = target.exists();
        if existed {
            let old = install::read_manifest_of(&target).map_err(|e| {
                CoreError::Internal(format!("已安装插件的 manifest 不可读，拒绝覆盖: {e}"))
            })?;
            if !install::version_gt(&manifest.version, &old.version) {
                return Err(CoreError::Internal(format!(
                    "升级版本必须高于当前版本（当前 {}，新 {}）",
                    old.version, manifest.version
                )));
            }
        }

        // 卸下现役句柄（持调用锁防「执行中 dlclose」）
        self.unload_handle(&id);

        let staged = install::stage(src, &self.root)?;
        let backup = match install::commit_swap(&self.root, &manifest, &staged) {
            Ok(b) => b,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staged);
                return Err(e.into());
            }
        };

        match self.load_dir(&target) {
            Ok((_, kind)) => {
                let version = manifest.version.clone();
                let enabled = self.db_enabled(&id).unwrap_or(true);
                plugin_repo::upsert(
                    &self.db,
                    &PluginRow {
                        id: id.clone(),
                        version: version.clone(),
                        plugin_type: manifest.plugin_type.clone(),
                        path: target.to_string_lossy().into_owned(),
                        enabled,
                        abi: manifest.abi,
                        transport: manifest.transport().to_string(),
                    },
                )?;
                self.loaded
                    .lock()
                    .expect("plugin map lock")
                    .insert(id.clone(), kind);
                self.set_last_error(&id, None);
                // 只保留最新一份备份供回滚
                let keep = backup.clone();
                install::prune_backups(&self.root, &id, keep.as_deref())?;
                self.emit(
                    &id,
                    if existed { "upgraded" } else { "installed" },
                    Some(&version),
                );
                tracing::info!(plugin = %id, version = %version, existed, "插件安装/升级完成");
            }
            Err(e) => {
                // 加载验证失败 → 回滚
                let rollback_note = if let Some(b) = &backup {
                    let _ = std::fs::remove_dir_all(&target);
                    if std::fs::rename(b, &target).is_ok() {
                        match self.load_dir(&target) {
                            Ok((_, kind)) => {
                                self.loaded
                                    .lock()
                                    .expect("plugin map lock")
                                    .insert(id.clone(), kind);
                                install::prune_backups(&self.root, &id, None).ok();
                                format!("已自动回滚到旧版本 {}", old_version_of(&self.root, &id))
                            }
                            Err(e2) => format!(
                                "旧版本恢复后加载也失败（{e2}），插件已禁用，请检查插件目录"
                            ),
                        }
                    } else {
                        "旧版本恢复失败，请手动检查插件目录".to_string()
                    }
                } else {
                    let _ = std::fs::remove_dir_all(&target);
                    "首次安装失败，已清理残留".to_string()
                };
                self.set_last_error(&id, Some(e.clone()));
                self.emit(&id, "install-failed", Some(&manifest.version));
                return Err(CoreError::Internal(format!(
                    "插件 {} 加载验证失败: {e}；{rollback_note}",
                    manifest.version
                )));
            }
        }
        self.list()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| CoreError::Internal("安装后列表查询失败".into()))
    }

    /// 回滚到最近一次升级前的版本（备份 ↔ 当前互换）
    pub fn rollback(&self, id: &str) -> CoreResult<PluginView> {
        self.unload_handle(id);
        let manifest = install::rollback(&self.root, id)?;
        let target = install::target_dir(&self.root, id);
        match self.load_dir(&target) {
            Ok((_, kind)) => {
                let version = manifest.version.clone();
                plugin_repo::upsert(
                    &self.db,
                    &PluginRow {
                        id: id.to_string(),
                        version: version.clone(),
                        plugin_type: manifest.plugin_type.clone(),
                        path: target.to_string_lossy().into_owned(),
                        enabled: self.db_enabled(id).unwrap_or(true),
                        abi: manifest.abi,
                        transport: manifest.transport().to_string(),
                    },
                )?;
                self.loaded
                    .lock()
                    .expect("plugin map lock")
                    .insert(id.to_string(), kind);
                self.set_last_error(id, None);
                self.emit(id, "rolled-back", Some(&version));
            }
            Err(e) => {
                self.set_last_error(id, Some(e.clone()));
                return Err(CoreError::Internal(format!(
                    "回滚完成但加载失败: {e}；请刷新扫描或检查插件目录"
                )));
            }
        }
        self.list()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| CoreError::Internal("回滚后列表查询失败".into()))
    }

    /// 卸载：停句柄、删目录（含备份）、清注册行
    pub fn uninstall(&self, id: &str) -> CoreResult<()> {
        self.unload_handle(id);
        install::uninstall(&self.root, id)?;
        plugin_repo::remove(&self.db, id)?;
        self.set_last_error(id, None);
        self.emit(id, "uninstalled", None);
        Ok(())
    }

    /// 启停。禁用 = shutdown/unload（目录保留）；启用 = 重新加载（失败自动回落禁用态）。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> CoreResult<PluginView> {
        let row = plugin_repo::get(&self.db, id)?.ok_or_else(|| {
            CoreError::Internal(format!("插件不存在（未注册）: {id}，请先刷新扫描"))
        })?;
        if enabled {
            let dir = PathBuf::from(&row.path);
            match self.load_dir(&dir) {
                Ok((_, kind)) => {
                    plugin_repo::set_enabled(&self.db, id, true)?;
                    self.loaded
                        .lock()
                        .expect("plugin map lock")
                        .insert(id.to_string(), kind);
                    self.set_last_error(id, None);
                    self.emit(id, "enabled", Some(&row.version));
                }
                Err(e) => {
                    plugin_repo::set_enabled(&self.db, id, false)?;
                    self.set_last_error(id, Some(e.clone()));
                    self.emit(id, "faulted", None);
                    return Err(CoreError::Internal(format!("启用失败: {e}")));
                }
            }
        } else {
            self.unload_handle(id);
            plugin_repo::set_enabled(&self.db, id, false)?;
            self.emit(id, "disabled", Some(&row.version));
        }
        self.list()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| CoreError::Internal("启停后列表查询失败".into()))
    }

    /// 卸下内存句柄（持调用锁；drop 触发 shutdown；process 插件 stop）
    fn unload_handle(&self, id: &str) {
        let mut guard = self.loaded.lock().expect("plugin map lock");
        loader::with_call_lock(|| {
            if let Some(kind) = guard.remove(id) {
                match kind {
                    LoadedKind::InProcess(p) => drop(p), // Drop → at_plugin_shutdown
                    LoadedKind::Process(p, _) => p.stop(),
                }
            }
        });
    }

    /// 卸载全部插件（shutdown）；进程退出前用。
    pub fn unload_all(&self) {
        let mut guard = self.loaded.lock().expect("plugin map lock");
        loader::with_call_lock(|| {
            // quarantine 里的句柄绝不 drop（可能仍在执行中）；仅清空活跃句柄
            guard.clear();
        });
    }
}

fn old_version_of(root: &Path, id: &str) -> String {
    install::read_manifest_of(&install::target_dir(root, id))
        .map(|m| m.version)
        .unwrap_or_else(|_| "未知".to_string())
}

#[derive(Debug, Clone)]
pub struct ScanReport {
    pub loaded: Vec<String>,
    pub errors: Vec<PluginError>,
}

#[derive(Debug, Clone)]
pub struct PluginError {
    pub dir: String,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbiInfoView {
    pub abi_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
}

impl From<&AbiInfo> for AbiInfoView {
    fn from(a: &AbiInfo) -> Self {
        Self {
            abi_version: a.abi_version,
            id: a.id.clone(),
            name: a.name.clone(),
            version: a.version.clone(),
            plugin_type: a.plugin_type.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub abi: u32,
    /// in-process | process
    pub transport: String,
    pub enabled: bool,
    pub loaded: bool,
    pub dir: Option<String>,
    pub capabilities: Vec<String>,
    /// 动态库/进程自报的 ABI 信息（未加载时为 null）
    pub abi_info: Option<AbiInfoView>,
    /// 是否存在可回滚的升级前备份
    pub rollback_available: bool,
    /// 最近一次故障信息（禁用/加载失败/超时等）
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingSink {
        count: AtomicUsize,
        phases: Mutex<Vec<String>>,
    }

    impl PluginEventSink for RecordingSink {
        fn plugin_changed(&self, _id: &str, phase: &str, _version: Option<&str>) {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.phases.lock().unwrap().push(phase.to_string());
        }
    }

    fn platform_key() -> String {
        current_platform_key()
    }

    /// 由测试可执行文件反推 cargo target profile 目录（同 tests/plugin_abi_e2e.rs）
    fn profile_dir() -> PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let deps = exe.parent().expect("deps dir");
        deps.parent().expect("profile dir").to_path_buf()
    }

    fn nested_build(args: &[&str]) {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let ws = profile_dir()
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let status = std::process::Command::new(&cargo)
            .args(args)
            .current_dir(&ws)
            .status()
            .expect("spawn cargo");
        assert!(status.success(), "嵌套构建失败: {args:?}");
    }

    /// 确保示例 cdylib 存在（dev-dep 只编 rlib，cdylib 需显式 build）
    fn ensure_cdylib() -> PathBuf {
        let (prefix, suffix) = if cfg!(windows) {
            ("", ".dll")
        } else if cfg!(target_os = "macos") {
            ("lib", ".dylib")
        } else {
            ("lib", ".so")
        };
        let path = profile_dir().join(format!("{prefix}crypto_base64{suffix}"));
        if !path.exists() {
            nested_build(&["build", "-p", "plugin-crypto-base64", "--lib"]);
        }
        assert!(path.exists(), "cdylib 缺失: {}", path.display());
        path
    }

    /// 确保 process-echo 可执行文件存在（P6 进程插件夹具）
    fn ensure_echo_bin() -> PathBuf {
        let path = profile_dir().join("process-echo");
        if !path.exists() {
            nested_build(&["build", "-p", "process-echo", "--bin", "process-echo"]);
        }
        assert!(path.exists(), "process-echo 缺失: {}", path.display());
        path
    }

    fn make_service(root: &Path) -> PluginService {
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db.clone()));
        PluginService::new(
            db,
            config,
            root.to_path_buf(),
            Arc::new(RecordingSink::default()),
        )
    }

    /// 组装插件源目录：manifest + 产物（拷贝自给定文件）
    fn stage_source(
        src_root: &Path,
        name: &str,
        manifest_text: &str,
        artifact: &Path,
        artifact_rel: &str,
    ) -> PathBuf {
        let dir = src_root.join(name);
        let rel = dir.join(artifact_rel);
        std::fs::create_dir_all(rel.parent().unwrap()).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest_text).unwrap();
        std::fs::copy(artifact, &rel).unwrap();
        dir
    }

    /// cdylib 产物路径与其在插件目录内的相对位置（entry 值）
    fn cdylib_artifact() -> (PathBuf, String) {
        let p = ensure_cdylib();
        let rel = format!(
            "{}/{}",
            platform_key(),
            p.file_name().unwrap().to_string_lossy()
        );
        (p, rel)
    }

    fn inproc_manifest(id: &str, version: &str, abi: u32) -> String {
        let plat = platform_key();
        let artifact_name = ensure_cdylib()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        format!(
            r#"{{"id":"{id}","name":"Base64","version":"{version}","abi":{abi},"type":"crypto",
                "entry":{{"{plat}":"{plat}/{artifact_name}"}},"capabilities":["encode","decode"]}}"#
        )
    }

    fn call_json(svc: &PluginService, id: &str, v: serde_json::Value) -> serde_json::Value {
        let (code, out) = svc.call(id, v.to_string().as_bytes()).expect("call 应成功");
        assert_eq!(code, 0, "业务返回码应为 0");
        serde_json::from_slice(&out).expect("输出应为 JSON")
    }

    #[test]
    fn full_flow_install_call_disable_enable_upgrade_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);

        // 1) 首次安装 1.0.0
        let (artifact, rel) = cdylib_artifact();
        let src = stage_source(
            &src_root,
            "p1",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &rel,
        );
        let view = svc.install(&src).unwrap();
        assert!(view.loaded && view.enabled);
        assert_eq!(view.version, "1.0.0");
        assert!(!view.rollback_available);

        // 2) 调用编码
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"hello"}),
        );
        assert_eq!(resp["ok"], true);

        // 3) 禁用 → 调用报「已禁用」；启用 → 恢复
        let v = svc.set_enabled("crypto.base64", false).unwrap();
        assert!(!v.enabled && !v.loaded);
        let err = svc.call("crypto.base64", b"{}").unwrap_err().to_string();
        assert!(err.contains("已禁用"), "{err}");
        svc.set_enabled("crypto.base64", true).unwrap();
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"x"}),
        );
        assert_eq!(resp["ok"], true);

        // 4) 升级 1.1.0 → 可回滚
        let (artifact, rel) = cdylib_artifact();
        let src2 = stage_source(
            &src_root,
            "p2",
            &inproc_manifest("crypto.base64", "1.1.0", 1),
            &artifact,
            &rel,
        );
        let view = svc.install(&src2).unwrap();
        assert_eq!(view.version, "1.1.0");
        assert!(view.rollback_available, "升级后应存在备份");

        // 5) 回滚 → 回到 1.0.0 且可用
        let view = svc.rollback("crypto.base64").unwrap();
        assert_eq!(view.version, "1.0.0");
        assert!(!view.rollback_available, "回滚后备份清空");
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"y"}),
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn install_rejects_downgrade_and_equal_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();

        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();
        // 同版本
        let s2 = stage_source(
            &src_root,
            "b",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        assert!(svc.install(&s2).is_err(), "同版本不允许覆盖");
        // 降级
        let s3 = stage_source(
            &src_root,
            "c",
            &inproc_manifest("crypto.base64", "0.9.0", 1),
            &artifact,
            &artifact_rel,
        );
        let err = svc.install(&s3).unwrap_err().to_string();
        assert!(err.contains("版本"), "{err}");
    }

    #[test]
    fn upgrade_with_abi_mismatch_is_rejected_before_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();

        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        // abi=2 的「新版」必须在替换前被拒（§9.4 ABI 不匹配拒绝）
        let s2 = stage_source(
            &src_root,
            "b",
            &inproc_manifest("crypto.base64", "2.0.0", 2),
            &artifact,
            &artifact_rel,
        );
        let err = svc.install(&s2).unwrap_err().to_string();
        assert!(err.contains("ABI") || err.contains("abi"), "{err}");
        // 旧版原封不动且可用
        let list = svc.list().unwrap();
        assert_eq!(list[0].version, "1.0.0");
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"z"}),
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn upgrade_load_failure_rolls_back_to_old_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();

        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        // 通过校验但加载必然失败：产物是垃圾字节（合法文件、非法动态库）
        let bad_manifest = inproc_manifest("crypto.base64", "1.1.0", 1);
        let bad_dir = src_root.join("bad");
        std::fs::create_dir_all(bad_dir.join(platform_key())).unwrap();
        std::fs::write(bad_dir.join("manifest.json"), bad_manifest).unwrap();
        std::fs::write(
            bad_dir
                .join(platform_key())
                .join(artifact.file_name().unwrap()),
            b"not a dylib",
        )
        .unwrap();

        let err = svc.install(&bad_dir).unwrap_err().to_string();
        assert!(err.contains("回滚"), "{err}");
        // 旧版被恢复且可用
        let list = svc.list().unwrap();
        assert_eq!(list[0].version, "1.0.0");
        assert!(list[0].loaded);
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"w"}),
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn uninstall_removes_dir_row_and_rejects_call() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();
        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        svc.uninstall("crypto.base64").unwrap();
        assert!(svc.list().unwrap().is_empty());
        assert!(!root.join("crypto.base64").exists(), "目录应被删除");
        let err = svc.call("crypto.base64", b"{}").unwrap_err().to_string();
        assert!(err.contains("不存在") || err.contains("未加载"), "{err}");
    }

    #[test]
    fn inprocess_panic_is_captured_and_host_survives() {
        // §9.4：故意 panic 的插件 → SDK 捕获为 -99，主程序不退出且可继续调用
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();
        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        let (code, out) = svc.call("crypto.base64", br#"{"op":"panic"}"#).unwrap();
        assert_eq!(code, -99, "panic 应被 SDK 捕获为 -99");
        let _ = serde_json::from_slice::<serde_json::Value>(&out); // 输出可为空
        // 宿主存活：随后正常调用成功
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"ok"}),
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn inprocess_timeout_faults_disables_and_keeps_host_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db.clone()));
        config
            .set(
                crate::services::config_service::KEY_PLUGIN_CALL_TIMEOUT_MS,
                "50",
            )
            .unwrap();
        let svc = PluginService::new(db, config, root.clone(), Arc::new(RecordingSink::default()));

        let (artifact, artifact_rel) = cdylib_artifact();
        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        // 睡 400ms > 超时 50ms → 超时错误
        let err = svc
            .call("crypto.base64", br#"{"op":"sleep","ms":400}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("超时"), "{err}");
        // 插件被自动禁用
        let view = &svc.list().unwrap()[0];
        assert!(!view.enabled, "超时后应自动禁用");
        assert!(
            view.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("超时")
        );
        // 等工作线程真正退出（in-process 无法抢占），宿主仍存活
        std::thread::sleep(std::time::Duration::from_millis(600));
        // 重新启用后恢复可用
        svc.set_enabled("crypto.base64", true).unwrap();
        let resp = call_json(
            &svc,
            "crypto.base64",
            serde_json::json!({"op":"encode","data":"back"}),
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn payload_limit_rejects_oversized_output() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db.clone()));
        config
            .set(
                crate::services::config_service::KEY_PLUGIN_MAX_PAYLOAD_KB,
                "1",
            )
            .unwrap();
        let svc = PluginService::new(db, config, root, Arc::new(RecordingSink::default()));

        let (artifact, artifact_rel) = cdylib_artifact();
        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();

        // 2KB 输入 → ~2.7KB 输出 > 1KB 上限
        let big = "A".repeat(2048);
        let req = format!(r#"{{"op":"encode","data":"{big}"}}"#);
        let err = svc
            .call("crypto.base64", req.as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("上限"), "{err}");
    }

    #[test]
    fn process_plugin_install_call_crash_isolation_and_disable() {
        // §9.4 崩溃隔离：进程插件崩溃 → 调用报错、宿主存活、下次调用自动重启
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let bin = ensure_echo_bin();
        let plat = platform_key();

        let manifest = format!(
            r#"{{"id":"tool.echo","name":"Echo","version":"0.2.0","abi":1,"type":"tool",
                "transport":"process","entry":{{"{plat}":"{plat}/process-echo"}}}}"#
        );
        let src = stage_source(
            &src_root,
            "echo",
            &manifest,
            &bin,
            &format!("{plat}/process-echo"),
        );
        let view = svc.install(&src).unwrap();
        assert!(view.loaded);
        assert_eq!(view.transport, "process");
        assert_eq!(view.abi_info.as_ref().unwrap().id, "tool.echo");

        // call echo
        let resp = call_json(
            &svc,
            "tool.echo",
            serde_json::json!({"op":"echo","data":"hi"}),
        );
        assert_eq!(resp["data"], "hi");

        // crash → 调用报错，宿主存活
        let err = svc
            .call("tool.echo", br#"{"op":"crash"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("崩溃"), "{err}");
        // 下次调用懒重启，恢复正常
        let resp = call_json(
            &svc,
            "tool.echo",
            serde_json::json!({"op":"upper","data":"abc"}),
        );
        assert_eq!(resp["data"], "ABC");

        // 禁用 → 明确报错
        svc.set_enabled("tool.echo", false).unwrap();
        let err = svc
            .call("tool.echo", br#"{"op":"echo","data":"x"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("已禁用"), "{err}");
    }

    #[test]
    fn scan_after_disable_registers_without_loading() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let src_root = tmp.path().join("src");
        let svc = make_service(&root);
        let (artifact, artifact_rel) = cdylib_artifact();
        let s1 = stage_source(
            &src_root,
            "a",
            &inproc_manifest("crypto.base64", "1.0.0", 1),
            &artifact,
            &artifact_rel,
        );
        svc.install(&s1).unwrap();
        svc.set_enabled("crypto.base64", false).unwrap();

        let report = svc.scan().unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        let list = svc.list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].enabled && !list[0].loaded, "禁用插件只注册不加载");
    }
}
