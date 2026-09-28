//! ConfigService：应用设置的读写与校验（P1）。
//! 业务规则集中在这里：合法键、值校验、默认值；Command 层只做转发。

use std::sync::Arc;

use crate::core::error::{CoreError, CoreResult};
use crate::db::{Db, config_repo};

pub const KEY_THEME: &str = "app.settings.theme";
pub const KEY_OPACITY: &str = "app.settings.opacity";
pub const KEY_LOG_LEVEL: &str = "app.settings.log_level";
/// 透明区点击穿透开关（第五十四轮）：开着它，界面上纯透明的留白处点击会落到后面的 App
pub const KEY_CLICK_THROUGH: &str = "app.settings.click_through";
/// 手动指定的 adb 路径（P3）；空 = 走环境变量自动探测
pub const KEY_ADB_PATH: &str = "app.adb.path";
/// Android Agent binary 显式路径；空 = 环境变量/resource/workspace 自动发现
pub const KEY_AGENT_PATH: &str = "app.agent.path";
/// 设备上线后自动把 Agent 接起来（AR12.5 / D086）。
///
/// 默认开。这一层只管「探测 + 按授权装机/启动」，**写入设备那一步仍受
/// `KEY_AGENT_CONSENT_SERIALS` 管**：D028 反对的是静默往设备写二进制，
/// 不是反对自动。关掉它 = 回到「必须人去点一下安装并连接」的老手感。
pub const KEY_AGENT_AUTO_CONNECT: &str = "app.agent.auto_connect";
/// 已授权「这台设备可以装/起 Agent」的 serial 列表（逗号分隔，空 = 一台都没授权）。
///
/// 为什么不是一台设备一个键：`ALLOWED_KEYS` 是**固定白名单**，`snapshot()` 靠它整包
/// 下发；动态键既进不了白名单，也没法被前端读到（更没法被设置页管理）。
pub const KEY_AGENT_CONSENT_SERIALS: &str = "app.agent.consent_serials";
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
/// Hook 脚本工作目录（P10 Frida 工作台）；空 = 未选择
pub const KEY_HOOK_WORKDIR: &str = "app.hook.workdir";
/// 翻译接口（UI-6 第二层的可选环节）。
///
/// ⚠️ `KEY_AI_API_KEY` 只存在桌面本机库里：不下发设备、不进 adb 命令行、不上日志、
/// 不进 Git。界面读配置时只拿得到后 4 位（见 `ai_service::AiConfigView`）。
pub const KEY_AI_BASE_URL: &str = "app.ai.base_url";
/// ⚠️ 不在 ALLOWED_KEYS 里（见那张表的注释）：不许被 snapshot 整包带进前端。
pub const KEY_AI_API_KEY: &str = "app.ai.api_key";
pub const KEY_AI_MODEL: &str = "app.ai.model";
pub const KEY_AI_ENABLED: &str = "app.ai.enabled";

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

/// 布尔型设置：只认 "true"/"false" 两个字面量（跟前端 JSON 序列化口径一致）。
/// 翻译接口地址：允许空（= 没配），非空必须是 https 或本机 http。
///
/// 这条校验和 `ai_service::validate_endpoint` 同一条规则，两处都要有：
/// 存进去一个明文远端地址，等于把 key 裸发出去，不能等到发请求那一刻才拒。
fn is_valid_ai_base_url(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return value.len() <= 512 && !value.chars().any(|c| c.is_ascii_whitespace());
    }
    let rest = lower.strip_prefix("http://").unwrap_or_default();
    lower.starts_with("http://")
        && (rest.starts_with("127.0.0.1")
            || rest.starts_with("localhost")
            || rest.starts_with("[::1]"))
}

/// 模型名：空或一段不带空白的短串（各家网关都按名字选模型，带空格一定是粘错了）
fn is_valid_ai_model(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || (value.len() <= 120 && !value.chars().any(|c| c.is_ascii_whitespace()))
}

fn is_valid_bool(v: &str) -> bool {
    matches!(v, "true" | "false")
}
/// 已授权 serial 列表：逗号分隔、去空白后每段只允许 adb serial 的合法字符。
///
/// 为什么要校验到这个程度：这个串会被拿去和用户可控的设备 serial 做比对，
/// 塞进换行/控制字符就等于给"配置值"开了一个注入面（长度上限同 `is_valid_path`）。
fn is_valid_serial_list(value: &str) -> bool {
    if value.len() >= 2000 || value.contains('\n') || value.contains('\0') {
        return false;
    }
    // 空串是合法值（= 撤销全部授权）；不认它就没法把最后一台设备的授权收回。
    if value.is_empty() {
        return true;
    }
    value.split(',').all(|item| {
        let item = item.trim();
        !item.is_empty()
            && item.len() <= 128
            && item
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    })
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
    (KEY_CLICK_THROUGH, is_valid_bool),
    (KEY_ADB_PATH, is_valid_path),
    (KEY_AGENT_PATH, is_valid_path),
    (KEY_AGENT_AUTO_CONNECT, is_valid_bool),
    (KEY_AGENT_CONSENT_SERIALS, is_valid_serial_list),
    (KEY_PLUGIN_CALL_TIMEOUT_MS, is_valid_timeout_ms),
    (KEY_PLUGIN_MAX_PAYLOAD_KB, is_valid_payload_kb),
    (KEY_PYTHON_PATH, is_valid_path),
    (KEY_NODE_PATH, is_valid_path),
    (KEY_IDA_MCP_PORT, is_valid_port),
    (KEY_JADX_MCP_PORT, is_valid_port),
    (KEY_LOCALE, is_valid_locale),
    (KEY_HOOK_WORKDIR, is_valid_path),
    // 翻译接口（UI-6 第二层）。这三个键的值都不敏感：地址、模型名、开关。
    (KEY_AI_BASE_URL, is_valid_ai_base_url),
    (KEY_AI_MODEL, is_valid_ai_model),
    (KEY_AI_ENABLED, is_valid_bool),
    // ⚠️ API key 故意**不进** `ALLOWED_KEYS`：`snapshot()` 就是靠这张表把配置
    // 整包发给前端的，key 一旦进表就会被前端读到（更糟的是被顺手打进日志）。
    // 它只能由 `ai_service` 自己按精确键名读写，对外只出后 4 位。
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

    /// 写**敏感键**：绕过 `ALLOWED_KEYS`，但绝不允许被 `snapshot()` 带走。
    ///
    /// 为什么要单独开一条：这张白名单存在的意义就是"前端启动时能整包拉走"，
    /// 而 API key 恰恰不能被整包拉走——它只能由 `ai_service` 按精确键名读写，
    /// 对外只出后 4 位。用同一个 `set()` 写它反而会撞上这张表（撞不出语义差别，
    /// 只会让人以为"这个键没注册"然后顺手把它加进表里，那才是真的漏）。
    pub fn set_secret(&self, key: &str, value: &str) -> CoreResult<()> {
        if key.trim().is_empty() {
            return Err(CoreError::Internal("敏感键名不能为空".to_string()));
        }
        config_repo::set(&self.db, key, value)
    }

    /// 读敏感键；缺失返回 default。
    pub fn get_secret(&self, key: &str, default: &str) -> CoreResult<String> {
        Ok(config_repo::get(&self.db, key)?.unwrap_or_else(|| default.to_string()))
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

    /// 纪律①的机械化：API key 绝不进 `snapshot()`。
    ///
    /// 前端启动时是**整包**拉配置的，所以这个键一旦被加进 `ALLOWED_KEYS`
    /// 就等于把 key 交给 webview（以及任何顺手打印 settings 的地方）。
    /// 这条测试故意写得像在"测试一个缺陷"：它守的是以后有人图省事把键加回表里。
    #[test]
    fn api_key_never_leaks_through_the_settings_snapshot() {
        let service = ConfigService::new(Arc::new(Db::in_memory().unwrap()));
        service
            .set(KEY_AI_BASE_URL, "https://api.example.com/v1")
            .expect("接口地址是普通键");
        service
            .set_secret(KEY_AI_API_KEY, "sk-should-never-appear-in-snapshot")
            .expect("key 走 secret 通道");
        let dumped = service
            .snapshot()
            .expect("snapshot 应可读")
            .iter()
            .map(|item| format!("{}={}", item.key, item.value))
            .collect::<Vec<_>>()
            .join(";");
        assert!(
            !dumped.contains("app.ai.api_key"),
            "key 的键名都不该出现在整包配置里：{dumped}"
        );
        assert!(
            !dumped.contains("sk-should-never-appear"),
            "key 的本体更不该出现：{dumped}"
        );
        // 普通 set 走不通这个键：想整包读它就得显式用 get_secret
        assert!(
            service.set(KEY_AI_API_KEY, "x").is_err(),
            "敏感键必须被白名单挡在 snapshot 之外"
        );
        assert!(service.get(KEY_AI_API_KEY, "").is_err(), "读也一样要显式");
        assert_eq!(
            service
                .get_secret(KEY_AI_API_KEY, "")
                .expect("secret 通道可读")
                .len(),
            "sk-should-never-appear-in-snapshot".len()
        );
    }

    use crate::db::Db;

    fn svc() -> ConfigService {
        ConfigService::new(Arc::new(Db::in_memory().unwrap()))
    }

    #[test]
    fn set_and_get_roundtrip() {
        let s = svc();
        s.set(KEY_THEME, "dark").unwrap();
        assert_eq!(s.get(KEY_THEME, "system").unwrap(), "dark");
        s.set(KEY_AGENT_PATH, "/tmp/android-agent").unwrap();
        assert_eq!(s.get(KEY_AGENT_PATH, "").unwrap(), "/tmp/android-agent");
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
