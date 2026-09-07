//! plugins 表访问层（插件注册表）。

use rusqlite::params;

use crate::core::error::{CoreError, CoreResult};
use crate::db::Db;

#[derive(Debug, Clone)]
pub struct PluginRow {
    pub id: String,
    pub version: String,
    pub plugin_type: String,
    pub path: String,
    pub enabled: bool,
    pub abi: u32,
}

pub fn upsert(db: &Db, row: &PluginRow) -> CoreResult<()> {
    let (id, version, plugin_type, path) = (
        row.id.clone(),
        row.version.clone(),
        row.plugin_type.clone(),
        row.path.clone(),
    );
    let (enabled, abi) = (row.enabled, row.abi as i64);
    db.with(move |conn| {
        conn.execute(
            "INSERT INTO plugins (id, version, type, path, enabled, abi) \
             VALUES (?1,?2,?3,?4,?5,?6) \
             ON CONFLICT(id) DO UPDATE SET version=excluded.version, type=excluded.type, \
             path=excluded.path, abi=excluded.abi",
            params![id, version, plugin_type, path, enabled, abi],
        )?;
        Ok(())
    })
}

/// 启停开关：P6 插件中心接入；P4 入库后默认启用
#[allow(dead_code)]
pub fn set_enabled(db: &Db, id: &str, enabled: bool) -> CoreResult<()> {
    let id = id.to_string();
    db.with(move |conn| {
        conn.execute(
            "UPDATE plugins SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    })
}

pub fn list(db: &Db) -> CoreResult<Vec<PluginRow>> {
    db.with(|conn| {
        let mut stmt =
            conn.prepare("SELECT id, version, type, path, enabled, abi FROM plugins ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(PluginRow {
                id: r.get(0)?,
                version: r.get(1)?,
                plugin_type: r.get(2)?,
                path: r.get(3)?,
                enabled: r.get::<_, i64>(4)? != 0,
                abi: r.get::<_, i64>(5)? as u32,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    })
}

/// 移除不再存在于受控目录的注册行（目录即事实源）
pub fn remove_except(db: &Db, keep: &[String]) -> CoreResult<usize> {
    let keep = keep.to_vec();
    db.with(move |conn| {
        let all: Vec<String> = conn
            .prepare("SELECT id FROM plugins")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut n = 0;
        for id in all.iter().filter(|id| !keep.contains(id)) {
            // 关联 algorithms 由外键级联清理
            n += conn.execute("DELETE FROM plugins WHERE id = ?1", params![id])?;
        }
        Ok(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn row(id: &str, enabled: bool) -> PluginRow {
        PluginRow {
            id: id.into(),
            version: "1.0.0".into(),
            plugin_type: "crypto".into(),
            path: format!("/plugins/{id}"),
            enabled,
            abi: 1,
        }
    }

    #[test]
    fn upsert_and_list() {
        let db = Db::in_memory().unwrap();
        upsert(&db, &row("crypto.base64", true)).unwrap();
        upsert(&db, &row("crypto.base64", true)).unwrap(); // 重复 upsert 不新增
        let rows = list(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].enabled);
    }

    #[test]
    fn set_enabled_roundtrip() {
        let db = Db::in_memory().unwrap();
        upsert(&db, &row("a.b", true)).unwrap();
        set_enabled(&db, "a.b", false).unwrap();
        assert!(!list(&db).unwrap()[0].enabled);
    }

    #[test]
    fn remove_except_deletes_stale() {
        let db = Db::in_memory().unwrap();
        upsert(&db, &row("keep.me", true)).unwrap();
        upsert(&db, &row("gone.me", true)).unwrap();
        let n = remove_except(&db, &["keep.me".to_string()]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(list(&db).unwrap().len(), 1);
    }
}
