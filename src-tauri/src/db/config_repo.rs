//! app_settings 表访问层：键值配置读写，值统一以字符串存储，JSON 序列化由调用方负责。

use rusqlite::params;

use crate::core::error::{CoreError, CoreResult};
use crate::db::Db;

pub fn get(db: &Db, key: &str) -> CoreResult<Option<String>> {
    let key = key.to_string();
    db.with(move |conn| {
        let mut stmt = conn.prepare("SELECT value FROM app_settings WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |r| r.get::<_, String>(0))?;
        match rows.next() {
            Some(v) => Ok(Some(v?)),
            None => Ok(None),
        }
    })
}

pub fn set(db: &Db, key: &str, value: &str) -> CoreResult<()> {
    let (key, value) = (key.to_string(), value.to_string());
    db.with(move |conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) \
             VALUES (?1, ?2, strftime('%s','now')) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, \
             updated_at = excluded.updated_at",
            params![key, value],
        )?;
        Ok(())
    })
}

/// 删除键：P1 测试覆盖 + P6 插件/预设清理时使用，允许暂时无生产调用点。
#[allow(dead_code)]
pub fn remove(db: &Db, key: &str) -> CoreResult<()> {
    let key = key.to_string();
    db.with(move |conn| {
        conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
        Ok(())
    })
}

pub fn all(db: &Db) -> CoreResult<Vec<(String, String)>> {
    db.with(|conn| {
        let mut stmt = conn.prepare("SELECT key, value FROM app_settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn set_then_get_returns_value() {
        let db = Db::in_memory().unwrap();
        set(&db, "app.settings.theme", "dark").unwrap();
        assert_eq!(
            get(&db, "app.settings.theme").unwrap().as_deref(),
            Some("dark")
        );
    }

    #[test]
    fn set_upserts_same_key() {
        let db = Db::in_memory().unwrap();
        set(&db, "k", "a").unwrap();
        set(&db, "k", "b").unwrap();
        assert_eq!(get(&db, "k").unwrap().as_deref(), Some("b"));
        assert_eq!(all(&db).unwrap().len(), 1, "upsert 不应新增行");
    }

    #[test]
    fn get_missing_key_returns_none() {
        let db = Db::in_memory().unwrap();
        assert_eq!(get(&db, "absent").unwrap(), None);
    }

    #[test]
    fn remove_deletes_key() {
        let db = Db::in_memory().unwrap();
        set(&db, "k", "v").unwrap();
        remove(&db, "k").unwrap();
        assert_eq!(get(&db, "k").unwrap(), None);
    }
}
