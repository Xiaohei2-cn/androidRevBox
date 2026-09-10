//! ConfigService：应用设置的读写与校验（P1）。
//! 业务规则集中在这里：合法键、值校验、默认值；Command 层只做转发。

use std::sync::Arc;

use crate::core::error::{CoreError, CoreResult};
use crate::db::{Db, config_repo};

pub const KEY_THEME: &str = "app.settings.theme";
pub const KEY_OPACITY: &str = "app.settings.opacity";
pub const KEY_LOG_LEVEL: &str = "app.settings.log_level";
/// 手动指定的 adb 路径（P3）；空 = 走环境变量自动探测
pub const KEY_ADB_PATH: &str = "app.adb.path";
/// 插件单次调用超时（毫秒，P6）；100–600000
pub const KEY_PLUGIN_CALL_TIMEOUT_MS: &str = "app.plugins.call_timeout_ms";
/// 插件输入/输出载荷上限（KB，P6）；1–65536
pub const KEY_PLUGIN_MAX_PAYLOAD_KB: &str = "app.plugins.max_payload_kb";
/// Python 解释器路径（P7）；空 = 未配置（Python 卡与 Frida 卡据此剪枝）
pub const KEY_PYTHON_PATH: &str = "app.python.path";
/// Node 可执行路径（P7）；空 = 走系统 PATH 探测
pub const KEY_NODE_PATH: &str = "app.node.path";
/// IDA MCP 服务端口（P7）；1–65535
pub const KEY_IDA_MCP_PORT: &str = "app.tools.ida_mcp_port";
/// jadx-gui MCP 服务端口（P7）；1–65535
pub const KEY_JADX_MCP_PORT: &str = "app.tools.jadx_mcp_port";
/// 界面语言（P8 多语言）；取值见 LOCALES，默认 zh-CN
pub const KEY_LOCALE: &str = "app.settings.locale";

/// 界面语言白名单（与前端 i18n 词典文件一一对应；新增语种在此登记）
pub const LOCALES: [&str; 5] = ["zh-CN", "en", "ru", "pt-BR", "ja"];

fn is_valid_locale(v: &str) -> bool {
    LOCALES.contains(&v)
}

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

/// adb 路径：允许空串（清空=回到自动探测），限制长度防误贴大文本
fn is_valid_path(v: &str) -> bool {
    v.len() < 1000 && !v.chars().any(|c| c == '\n' || c == '\0')
}

/// 插件调用超时：100ms–10min
fn is_valid_timeout_ms(v: &str) -> bool {
    v.parse::<u64>()
        .map(|n| (100..=600_000).contains(&n))
        .unwrap_or(false)
}

/// 插件载荷上限：1KB–64MB
fn is_valid_payload_kb(v: &str) -> bool {
    v.parse::<u64>()
        .map(|n| (1..=65_536).contains(&n))
        .unwrap_or(false)
}

/// MCP 服务端口：1–65535
fn is_valid_port(v: &str) -> bool {
    v.parse::<u16>().is_ok()
}

type ValueValidator = fn(&str) -> bool;

/// 允许前端读写的键白名单（防止任意键写入）
const ALLOWED_KEYS: &[(&str, ValueValidator)] = &[
    (KEY_THEME, is_valid_theme),
    (KEY_OPACITY, is_valid_opacity),
    (KEY_LOG_LEVEL, is_valid_log_level),
    (KEY_ADB_PATH, is_valid_path),
    (KEY_PLUGIN_CALL_TIMEOUT_MS, is_valid_timeout_ms),
    (KEY_PLUGIN_MAX_PAYLOAD_KB, is_valid_payload_kb),
    (KEY_PYTHON_PATH, is_valid_path),
    (KEY_NODE_PATH, is_valid_path),
    (KEY_IDA_MCP_PORT, is_valid_port),
    (KEY_JADX_MCP_PORT, is_valid_port),
    (KEY_LOCALE, is_valid_locale),
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
