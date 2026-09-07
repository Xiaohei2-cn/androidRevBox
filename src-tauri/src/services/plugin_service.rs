//! PluginService：插件发现/加载/生命周期/调用的业务层（P4 最小闭环）。
//! 受控目录 = app_data/plugins。启动 scan()：逐个加载 → 成功者 upsert 进 plugins 表
//! （enabled 保留用户既有选择），失败者仅记日志不入库；卸载插件时清注册表行。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::params;

use crate::core::error::{CoreError, CoreResult};
use crate::db::Db;
use crate::plugins::loader::{self, LoadedPlugin};
use crate::plugins::manifest::PluginManifest;
use crate::plugins::plugin_repo;

pub struct PluginService {
    db: Arc<Db>,
    root: PathBuf,
    loaded: Mutex<HashMap<String, Arc<LoadedPlugin>>>,
}

impl PluginService {
    pub fn new(db: Arc<Db>, root: PathBuf) -> Self {
        Self {
            db,
            root,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 扫描受控目录并加载全部合法插件；返回加载成功清单与失败明细。
    /// 幂等：重复扫描覆盖旧句柄（drop 旧 → shutdown）。
    pub fn scan(&self) -> CoreResult<ScanReport> {
        let (plugins, errors) = loader::discover_and_load(&self.root);
        let mut loaded_map = HashMap::new();
        let mut ok_ids = Vec::new();
        for (_, p) in plugins {
            let id = p.manifest().id.clone();
            ok_ids.push(id.clone());
            // 入库；enabled 默认 true（新插件即启用，用户在 P6 可关）
            let row = plugin_repo::PluginRow {
                id: id.clone(),
                version: p.manifest().version.clone(),
                plugin_type: p.manifest().plugin_type.clone(),
                path: p.dir().to_string_lossy().into_owned(),
                enabled: self.db_enabled(&id).unwrap_or(true),
                abi: p.manifest().abi,
            };
            plugin_repo::upsert(&self.db, &row)?;
            loaded_map.insert(id, Arc::new(p));
        }
        // 目录里消失的插件：清库、移出内存
        plugin_repo::remove_except(&self.db, &ok_ids)?;
        let loaded_ids: Vec<String> = loaded_map.keys().cloned().collect();
        *self.loaded.lock().expect("plugin map lock") = loaded_map;

        let errors = errors
            .into_iter()
            .map(|(dir, e)| PluginError {
                dir,
                error: e.to_string(),
            })
            .collect();
        Ok(ScanReport {
            loaded: loaded_ids,
            errors,
        })
    }

    fn db_enabled(&self, id: &str) -> Option<bool> {
        let id = id.to_string();
        self.db
            .with(move |conn| {
                let mut stmt = conn.prepare("SELECT enabled FROM plugins WHERE id = ?1")?;
                let mut rows = stmt.query_map(params![id], |r| r.get::<_, i64>(0))?;
                match rows.next() {
                    Some(v) => Ok(Some(v? != 0)),
                    None => Ok(None),
                }
            })
            .ok()
            .flatten()
    }

    /// 已加载插件的 manifest 列表（含动态库自报的 abiInfo）
    pub fn list(&self) -> CoreResult<Vec<PluginView>> {
        let guard = self.loaded.lock().expect("plugin map lock");
        let rows = plugin_repo::list(&self.db)?;
        let mut out = Vec::new();
        for r in rows {
            let lp = guard.get(&r.id);
            out.push(PluginView {
                id: r.id.clone(),
                name: lp.map(|p| p.manifest().name.clone()).unwrap_or_default(),
                version: r.version,
                plugin_type: r.plugin_type,
                abi: r.abi,
                enabled: r.enabled,
                loaded: lp.is_some(),
                dir: lp.map(|p| p.dir().to_string_lossy().into_owned()),
                capabilities: lp
                    .map(|p| p.manifest().capabilities.clone())
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    pub fn manifest_of(&self, id: &str) -> Option<PluginManifest> {
        self.loaded
            .lock()
            .expect("plugin map lock")
            .get(id)
            .map(|p| p.manifest().clone())
    }

    /// 调用插件：返回 (C 错误码, 输出字节)。未加载 → 错误。
    pub fn call(&self, id: &str, input: &[u8]) -> CoreResult<(i32, Vec<u8>)> {
        let arc = {
            let guard = self.loaded.lock().expect("plugin map lock");
            guard.get(id).cloned()
        };
        let plugin = arc.ok_or_else(|| CoreError::Internal(format!("插件未加载或不存在: {id}")))?;
        Ok(plugin.call(input))
    }

    /// 卸载全部插件（shutdown）；进程退出前或测试清理用。
    pub fn unload_all(&self) {
        let mut guard = self.loaded.lock().expect("plugin map lock");
        guard.clear(); // drop → 每个 LoadedPlugin 触发 shutdown
    }
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
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub abi: u32,
    pub enabled: bool,
    pub loaded: bool,
    pub dir: Option<String>,
    pub capabilities: Vec<String>,
}
