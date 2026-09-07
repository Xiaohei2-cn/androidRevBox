//! ConfigService：应用设置的读写与校验（P1）。
//! 业务规则集中在这里：合法键、值校验、默认值；Command 层只做转发。

use std::sync::Arc;

use crate::core::error::{CoreError, CoreResult};
use crate::db::{Db, config_repo};

pub const KEY_THEME: &str = "app.settings.theme";
pub const KEY_OPACITY: &str = "app.settings.opacity";
pub const KEY_LOG_LEVEL: &str = "app.settings.log_level";

fn is_valid_theme(v: &str) -> bool {
    matches!(v, "light" | "dark" | "system")
}

/// 不透明度：整数字符串，20–100（与前端钳制一致）
fn is_valid_opacity(v: &str) -> bool {
    v.parse::<f64>()
        .map(|n| (20.0..=100.0).contains(&n))
        .unwrap_or(false)
}

fn is_valid_log_level(v: &str) -> bool {
    matches!(v, "trace" | "debug" | "info" | "warn" | "error")
}

type ValueValidator = fn(&str) -> bool;

/// 允许前端读写的键白名单（防止任意键写入）
const ALLOWED_KEYS: &[(&str, ValueValidator)] = &[
    (KEY_THEME, is_valid_theme),
    (KEY_OPACITY, is_valid_opacity),
    (KEY_LOG_LEVEL, is_valid_log_level),
];

#[derive(Clone)]
pub struct ConfigService {
    pub(crate) db: Arc<Db>,
}

impl ConfigService {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// 读一个键；缺失时返回 default。
    pub fn get(&self, key: &str, default: &str) -> CoreResult<String> {
        self.check_key_allowed(key)?;
        Ok(config_repo::get(&self.db, key)?.unwrap_or_else(|| default.to_string()))
    }

    /// 写一个键；校验值合法性。
    pub fn set(&self, key: &str, value: &str) -> CoreResult<()> {
        self.check_key_allowed(key)?;
        let valid = ALLOWED_KEYS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, validator)| validator(value))
            .unwrap_or(false);
        if !valid {
            return Err(CoreError::Internal(format!("非法的配置值: {key}={value}")));
        }
        config_repo::set(&self.db, key, value)
    }

    /// 读全部白名单键（前端启动时拉取；缺失键不返回，由前端用默认值）。
    pub fn snapshot(&self) -> CoreResult<Vec<crate::models::config::AppSettingDto>> {
        let rows = config_repo::all(&self.db)?;
        let allowed: Vec<&str> = ALLOWED_KEYS.iter().map(|(k, _)| *k).collect();
        Ok(rows
            .into_iter()
            .filter(|(k, _)| allowed.contains(&k.as_str()))
            .map(Into::into)
            .collect())
    }

    fn check_key_allowed(&self, key: &str) -> CoreResult<()> {
        if ALLOWED_KEYS.iter().any(|(k, _)| *k == key) {
            Ok(())
        } else {
            Err(CoreError::Internal(format!("未注册的配置键: {key}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn svc() -> ConfigService {
        ConfigService::new(Arc::new(Db::in_memory().unwrap()))
    }

    #[test]
    fn set_and_get_roundtrip() {
        let s = svc();
        s.set(KEY_THEME, "dark").unwrap();
        assert_eq!(s.get(KEY_THEME, "system").unwrap(), "dark");
    }

    #[test]
    fn missing_key_returns_default() {
        let s = svc();
        assert_eq!(s.get(KEY_THEME, "system").unwrap(), "system");
    }

    #[test]
    fn rejects_invalid_value() {
        let s = svc();
        assert!(s.set(KEY_THEME, "neon").is_err());
        assert!(s.set(KEY_OPACITY, "5").is_err());
        assert!(s.set(KEY_OPACITY, "80").is_ok());
    }

    #[test]
    fn rejects_unregistered_key() {
        let s = svc();
        assert!(s.set("rm -rf", "everything").is_err());
    }

    #[test]
    fn snapshot_only_contains_whitelisted_keys() {
        let s = svc();
        s.set(KEY_THEME, "light").unwrap();
        s.set(KEY_OPACITY, "90").unwrap();
        // 手工写入一个非白名单键，snapshot 应过滤掉
        s.db.with(|c| {
            c.execute(
                "INSERT INTO app_settings(key, value) VALUES('rogue','x')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let snap = s.snapshot().unwrap();
        assert_eq!(snap.len(), 2);
        assert!(snap.iter().all(|r| r.key != "rogue"));
    }
}
