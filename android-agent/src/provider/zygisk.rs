//! Agent 侧 Zygisk Provider。
//!
//! 职责边界（AR5.3 契约）：模块模板源码零改动，Agent 负责连接模块 bridge、
//! 把私有 `Q/E/D` 子协议转成公共 typed DTO、施加超时/体积上限与生命周期诊断。
//! Desktop 只调用本 Provider 注册的方法，不再直连模块端口。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agent_protocol::method::{
    PACKAGE_EXPORT_APK, PACKAGE_EXPORT_CLEAN, PACKAGE_LIST_LOCALIZED, ZYGISK_STATUS,
};
use agent_protocol::{
    AgentError, ErrorCode, LabelSource, LocalizedPackageItem, ModuleHandlerInfo,
    PackageExportApkParams, PackageExportApkResult, PackageExportCleanParams,
    PackageExportCleanResult, PackageListLocalizedParams, PackageListLocalizedResult, PackageScope,
    PackageWarning, ProviderHealth, ProviderInfo, StagedApkFile, ZygiskLifecycle,
    ZygiskStatusResult,
};
use serde::Deserialize;
use serde_json::{Value, to_value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const ZYGISK_METHODS: &[&str] = &[
    ZYGISK_STATUS,
    PACKAGE_LIST_LOCALIZED,
    PACKAGE_EXPORT_APK,
    PACKAGE_EXPORT_CLEAN,
];

/// 学习 demo 模块（v1 线协议 `Q/E/D`，端口 11500，无鉴权）。
pub const DEMO_MODULE_ID: &str = "applist";
/// 自有基线模块（v2 线协议，端口 11501，令牌握手 + 指定 locale）。
pub const PRO_MODULE_ID: &str = "applistpro";
pub const PRO_SUB_PROTOCOL_VERSION: u32 = 2;
pub const DEMO_SUB_PROTOCOL_VERSION: u32 = 1;

/// v2 握手能力位（AR10.1）。名字两边共用同一份字符串，别在模块侧另拼一套。
/// 新增方法时：模块宣告它 → 这里登记 → 调用点传进去，三处齐了才算接完。
pub const CAP_LIST: &str = "list";
pub const CAP_MANIFEST: &str = "manifest";
pub const CAP_EXPORT: &str = "export";
/// 模块自描述注册表的能力名（AR10.2）。旧模块不宣告它，`module_handlers` 就是空数组。
pub const CAP_HANDLERS: &str = "handlers";

/// 兼容旧常量名：AR5.3 台账与文档里 v1 冻结为子协议 1。
pub const MODULE_ID: &str = DEMO_MODULE_ID;
pub const SUB_PROTOCOL_VERSION: u32 = DEMO_SUB_PROTOCOL_VERSION;

const MODULE_HOST: &str = "127.0.0.1";
const MODULE_PORT: u16 = 11_500;
const PRO_PORT: u16 = 11_501;
const DEMO_PORT: u16 = MODULE_PORT;
/// 令牌文件的**新**位置：模块目录之外。`ksud module install` 会覆盖模块目录，
/// 而开机后模块目录里的"外来文件"还可能被清理——真机上就出现过令牌文件几分钟后消失、
/// v2 谁都进不来、Agent 一路退回 v1。运行期状态不该住在安装器拥有的目录里。
const PRO_TOKEN_PATH: &str = "/data/adb/applistpro.token";
/// 旧位置，只为"新 Agent + 尚未升级的模块"保留一次兜底读，不是首选。
const PRO_TOKEN_PATH_LEGACY: &str = "/data/adb/modules/applistpro/token";
const PRO_TOKEN_TTL: Duration = Duration::from_secs(300);
/// 模块内部响应缓冲 4 MiB，留出余量后作为单行上限。
const MAX_LINE_BYTES: u64 = 6 * 1024 * 1024;
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const STAGING_ROOT: &str = "/data/local/tmp/app-reverse-tools-apk";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const LINE_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TTL: Duration = Duration::from_secs(5);
/// 单次探测的**兜底**预算：只用来挡住"某一步没挂上超时"这种意外，不是正常上限。
///
/// 为什么必须明显大于内层各步之和：外层 `timeout` 一旦真的取消 `probe_now()`，
/// 它就再也走不到 `store()` —— 缓存永远是空的，于是每次请求都重探一次、每次都超时，
/// 并且 capability 检查会永远停在"尚未探测完成"（这个坑我今天正是踩在 status 已经
/// 变快、list 却被拒的时候）。正常上限由内层各步保证：su 2.5 s ×2、connect 0.8 s ×2、
/// 握手/注册表 2 s ×2、getprop 若干，合计不超过约 7 s；桌面端状态查询给的是 20 s。
const PROBE_BUDGET: Duration = Duration::from_secs(9);
const ROOT_TIMEOUT: Duration = Duration::from_millis(2500);
/// 探测路径上的单次等待上限（握手、注册表查询）。`LINE_TIMEOUT` 那 60 s 是给数据流式
/// 响应用的（清单几千行、APK 分帧），拿它等一次握手等于没有超时。
const PROBE_IO_TIMEOUT: Duration = Duration::from_secs(2);

pub struct ZygiskProvider {
    probe: Mutex<Option<Probe>>,
    pro_token: Mutex<Option<(String, Instant)>>,
    /// 探测单飞锁：并发请求共享一次探测，而不是各起一遍（每次都要 `su`，慢的时候
    /// 能把整个请求预算吃光）。用 tokio 的异步互斥，std Mutex 不能跨 await 持有。
    probe_flight: tokio::sync::Mutex<()>,
}

/// Agent 实际要用的模块通道。优先级：v2 自有模块 > v1 demo > 不可用。
/// v2 模块探测结果：端口可达之外，还要区分「拿得到令牌」与「握手真的成功」。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProState {
    Ready(ProHello),
    NeedsToken,
    HandshakeFailed(String),
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    variant: Variant,
    pro_locale: Option<String>,
    note: Option<String>,
}

/// 通道选择矩阵：v2 握手成功优先；v2 因缺 root 令牌或握手失败不可用时，
/// 若 demo v1 活着就显式退回（note 说明指定 locale 不可用），两者都不行才 Absent。
fn select_variant(pro: ProState, demo_alive: bool) -> Selection {
    match pro {
        ProState::Ready(hello) => Selection {
            variant: Variant::Pro,
            pro_locale: Some(hello.locale),
            note: None,
        },
        ProState::NeedsToken if demo_alive => Selection {
            variant: Variant::Demo,
            pro_locale: None,
            note: Some(
                "v2 模块在监听但 Agent 读不到令牌（需要 root），已退回 v1 demo 通道：指定 locale 与 labelSource 证据不可用"
                    .to_owned(),
            ),
        },
        ProState::NeedsToken => Selection {
            variant: Variant::ProLocked,
            pro_locale: None,
            note: None,
        },
        ProState::HandshakeFailed(reason) if demo_alive => Selection {
            variant: Variant::Demo,
            pro_locale: None,
            note: Some(format!("{reason}；已退回 v1 demo 通道，指定 locale 不可用")),
        },
        ProState::HandshakeFailed(reason) => Selection {
            variant: Variant::ProLocked,
            pro_locale: None,
            note: Some(reason),
        },
        ProState::Dead if demo_alive => Selection {
            variant: Variant::Demo,
            pro_locale: None,
            // 「v2 从来没装」也是一种不可用，理由同样必须给出来：UI 上要能区分
            // 「装了但拿不到令牌」与「压根没装」，否则用户不知道该去装模块还是去授权
            note: Some(
                "未检测到 v2 模块（未安装或未监听），已退回 v1 demo 通道：指定 locale 与 labelSource 证据不可用"
                    .to_owned(),
            ),
        },
        ProState::Dead => Selection {
            variant: Variant::Absent,
            pro_locale: None,
            note: None,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    /// v2 可达且令牌可用：清单/导出走 applistpro
    Pro,
    /// v2 端口活但读不到令牌（Agent 非 root）：不能假装能鉴权
    ProLocked,
    /// 只有 demo 模块
    Demo,
    /// 两个都不活
    Absent,
}

impl Variant {
    fn sub_protocol(self) -> u32 {
        match self {
            Self::Pro | Self::ProLocked => PRO_SUB_PROTOCOL_VERSION,
            Self::Demo => DEMO_SUB_PROTOCOL_VERSION,
            Self::Absent => 0,
        }
    }

    fn module_id(self) -> &'static str {
        match self {
            Self::Pro | Self::ProLocked => PRO_MODULE_ID,
            Self::Demo => DEMO_MODULE_ID,
            Self::Absent => "",
        }
    }
}

/// v2 `L` 帧条目（模块侧 camelCase）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProItem {
    pkg: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    label_source: String,
    #[serde(default)]
    resolved_locale: Option<String>,
    #[serde(default)]
    fallback_reason: Option<String>,
    #[serde(default)]
    version_name: String,
    #[serde(default)]
    version_code: Option<i64>,
    #[serde(default)]
    uid: Option<u32>,
    #[serde(default)]
    is_system: bool,
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Default)]
struct ModuleFacts {
    installed: bool,
    pending_update: bool,
    disabled_marker: bool,
    remove_marker: bool,
    version: Option<String>,
    version_code: Option<u32>,
}

#[derive(Debug, Clone, Default)]
struct RootFacts {
    pro: ModuleFacts,
    demo: ModuleFacts,
    zygisk_impl: Option<String>,
}

/// 令牌是敏感物：只缓存在内存里，绝不进 Probe/日志/错误信息。
#[derive(Debug, Clone)]
struct Probe {
    at: Instant,
    lifecycle: ZygiskLifecycle,
    variant: Variant,
    bridge_ready: bool,
    root: Option<RootFacts>,
    device_locale: Option<String>,
    pro_locale: Option<String>,
    probe_latency_ms: u64,
    /// 模块自描述的 handler 注册表（capability `handlers` 才有；旧模块为空）
    handlers: Vec<ModuleHandlerInfo>,
    detail: Option<String>,
}

impl Default for ZygiskProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ZygiskProvider {
    pub fn new() -> Self {
        Self {
            probe: Mutex::new(None),
            pro_token: Mutex::new(None),
            probe_flight: tokio::sync::Mutex::new(()),
        }
    }

    /// Agent 启动后先探一次，保证首个 `system.hello` 就带真实可用性。
    pub async fn warm_up(&self) {
        // 走 probe()：与真实请求共享同一把单飞锁和同一个预算，不各探一遍。
        self.probe().await;
    }

    fn cached(&self) -> Option<Probe> {
        self.probe.lock().ok().and_then(|guard| guard.clone())
    }

    fn store(&self, probe: Probe) {
        if let Ok(mut guard) = self.probe.lock() {
            *guard = Some(probe);
        }
    }

    async fn probe_now(&self) -> Probe {
        let started = Instant::now();
        // v2 优先：端口活 + 拿得到令牌 + 握手真的成功，才算可用（不是“文件装着”）。
        // v2 探测：端口 -> 令牌 -> 真实握手，三段都过才算可用。
        let pro_state = if connect_port(PRO_PORT, CONNECT_TIMEOUT).await.is_err() {
            ProState::Dead
        } else {
            match self.pro_token().await {
                None => ProState::NeedsToken,
                Some(token) => match self.handshake_with_refresh_retry(&token).await {
                    Ok(mut hello) => {
                        // 模块宣告 `handlers` 才追问注册表；旧模块不认 `I`，
                        // 这里失败必须静默——status 不能因为"多问一句"而变红。
                        if hello.capabilities.iter().any(|c| c == CAP_HANDLERS)
                            && let Ok(Ok(list)) = tokio::time::timeout(
                                PROBE_IO_TIMEOUT,
                                fetch_module_handlers(&token),
                            )
                            .await
                        {
                            hello.handlers = list;
                        }
                        ProState::Ready(hello)
                    }
                    Err(error) => ProState::HandshakeFailed(error.message),
                },
            }
        };
        let pro_handlers = match &pro_state {
            ProState::Ready(hello) => hello.handlers.clone(),
            _ => Vec::new(),
        };
        let demo_alive = !matches!(pro_state, ProState::Ready(_))
            && bridge_alive(DEMO_PORT, CONNECT_TIMEOUT).await;
        let selected = select_variant(pro_state, demo_alive);
        let variant = selected.variant;
        let pro_locale = selected.pro_locale;
        let pro_error = selected.note;
        let bridge_ready = matches!(variant, Variant::Pro | Variant::Demo);
        let root = probe_root().await;
        let device_locale = detect_device_locale().await;
        let mut lifecycle = classify_lifecycle(variant, root.as_ref());
        if let Some(reason) = pro_error {
            lifecycle.1 = Some(format!("{reason}；v2 握手未通过"));
        }
        let probe = Probe {
            at: Instant::now(),
            lifecycle: lifecycle.0,
            variant,
            bridge_ready,
            root,
            device_locale,
            pro_locale,
            probe_latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            handlers: pro_handlers,
            detail: lifecycle.1,
        };
        self.store(probe.clone());
        probe
    }

    /// 令牌只驻留内存（不进 Probe/日志/错误信息）。
    ///
    /// 不经缓存直接从模块目录读：`ksud module install` 会覆盖模块目录，令牌随之换人，
    /// 缓存里那个当场作废——这正是"装完模块不重启就连不上"的另一半原因。
    async fn read_pro_token(&self) -> Option<String> {
        // 先读新位置；读不到再试旧位置一次（老模块只写旧位置）。两者都读不到才算没令牌。
        match Self::read_token_file(PRO_TOKEN_PATH).await {
            Some(token) => Some(token),
            None => Self::read_token_file(PRO_TOKEN_PATH_LEGACY).await,
        }
    }

    async fn read_token_file(path: &str) -> Option<String> {
        let script = format!("cat {path}");
        let output = with_timeout(
            ROOT_TIMEOUT,
            Command::new("su").args(["-c", &script]).output(),
        )
        .await
        .ok()?
        .ok()?;
        if !output.status.success() {
            return None;
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let valid = token.len() == 128 && token.chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return None;
        }
        Some(token)
    }

    /// 带 300 s 缓存的读取：正常路径不反复起 su。
    async fn pro_token(&self) -> Option<String> {
        let cached = self
            .pro_token
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .filter(|(_, at)| at.elapsed() < PRO_TOKEN_TTL);
        if let Some((token, _)) = cached {
            return Some(token);
        }
        let token = self.read_pro_token().await?;
        self.cache_pro_token(&token);
        Some(token)
    }

    fn cache_pro_token(&self, token: &str) {
        if let Ok(mut guard) = self.pro_token.lock() {
            *guard = Some((token.to_owned(), Instant::now()));
        }
    }

    /// 握手失败时按磁盘重读一次再试：模块刚升级过就靠这一步救回来，
    /// 不需要重启手机，也不需要等缓存过期。
    async fn handshake_with_refresh_retry(&self, token: &str) -> Result<ProHello, AgentError> {
        match handshake_pro(token).await {
            Ok(hello) => Ok(hello),
            Err(first) => match self.read_pro_token().await {
                Some(fresh) if fresh != token => match handshake_pro(&fresh).await {
                    Ok(hello) => {
                        self.cache_pro_token(&fresh);
                        Ok(hello)
                    }
                    Err(_) => Err(first),
                },
                _ => Err(first),
            },
        }
    }

    async fn probe(&self) -> Probe {
        if let Some(cached) = self.cached() {
            if cached.at.elapsed() < PROBE_TTL {
                return cached;
            }
        }
        // 单飞 + 双检：拿到锁时可能已经有别的请求探测完了，直接用它的结果。
        let _flight = self.probe_flight.lock().await;
        if let Some(cached) = self.cached() {
            if cached.at.elapsed() < PROBE_TTL {
                return cached;
            }
        }
        // 总预算：探测里有 su、TCP、模块握手等外部依赖，任何一格慢都不能把调用方拖死。
        // 超时后不谎报"可用/不可用"：有旧结果就带着"这次没探完"的说明继续用，
        // 没有旧结果就如实报未判定。
        match tokio::time::timeout(PROBE_BUDGET, self.probe_now()).await {
            Ok(probe) => probe,
            Err(_) => match self.cached() {
                Some(stale) => Probe {
                    detail: Some(format!(
                        "本次探测超过 {:?} 未完成，沿用 {:?} 前的结果（root/模块状态可能已变化）",
                        PROBE_BUDGET,
                        stale.at.elapsed()
                    )),
                    ..stale
                },
                None => Probe {
                    at: Instant::now(),
                    lifecycle: ZygiskLifecycle::Faulted,
                    variant: Variant::Absent,
                    bridge_ready: false,
                    root: None,
                    device_locale: None,
                    pro_locale: None,
                    probe_latency_ms: PROBE_BUDGET.as_millis() as u64,
                    handlers: Vec::new(),
                    detail: Some(format!(
                        "探测超时（{:?}）：无法判定 root 与模块通道，不把未知当成不可用",
                        PROBE_BUDGET
                    )),
                },
            },
        }
    }

    fn status(&self, probe: &Probe) -> ZygiskStatusResult {
        let facts = probe.root.as_ref();
        let module = match probe.variant {
            Variant::Pro | Variant::ProLocked => facts.map(|f| &f.pro),
            Variant::Demo => facts.map(|f| &f.demo),
            Variant::Absent => facts.map(|f| if f.pro.installed { &f.pro } else { &f.demo }),
        };
        let module_id = match probe.variant {
            Variant::Pro | Variant::ProLocked => Some(PRO_MODULE_ID),
            Variant::Demo => Some(DEMO_MODULE_ID),
            Variant::Absent => facts.and_then(|f| {
                if f.pro.installed {
                    Some(PRO_MODULE_ID)
                } else if f.demo.installed {
                    Some(DEMO_MODULE_ID)
                } else {
                    None
                }
            }),
        };
        ZygiskStatusResult {
            lifecycle: probe.lifecycle,
            bridge_ready: probe.bridge_ready,
            root_available: probe.root.is_some(),
            module_id: module_id.map(str::to_owned),
            module_version: module.and_then(|m| m.version.clone()),
            module_version_code: module.and_then(|m| m.version_code),
            zygisk_impl: facts.and_then(|f| f.zygisk_impl.clone()),
            device_locale: probe.device_locale.clone(),
            sub_protocol_version: probe.variant.sub_protocol(),
            probe_latency_ms: Some(probe.probe_latency_ms),
            module_handlers: probe.handlers.clone(),
            detail: probe.detail.clone(),
        }
    }

    async fn handle_status(&self, params: Value) -> Result<Value, AgentError> {
        let _probe_params: agent_protocol::ZygiskStatusParams = parse_params(params)?;
        // 走 probe()，不要直接 probe_now()：这里绕过预算与单飞，正是"查状态能把整个
        // 请求拖到超时"的原因——一次探测要起 su、连模块、握两次手，任何一格慢都不该
        // 让 UI 干等；并发时也不该各探一遍。
        let probe = self.probe().await;
        serialize(&self.status(&probe))
    }

    async fn handle_list(&self, params: Value) -> Result<Value, AgentError> {
        let params: PackageListLocalizedParams = parse_params(params)?;
        let probe = self.probe().await;
        require_bridge(&probe)?;

        if probe.variant == Variant::Pro {
            let token = self.pro_token().await.ok_or_else(|| {
                provider_unavailable(
                    "v2 模块令牌不可用（Agent 需要 root 才能读令牌）",
                    Some("pro_token_missing"),
                )
            })?;
            return self.handle_list_pro(&params, &token, &probe).await;
        }

        let apps = request_module_line(b"Q")
            .await
            .and_then(|line| parse_apps(&line));
        let apps = match apps {
            Ok(apps) => apps,
            Err(error) => {
                self.mark_faulted(&error);
                return Err(error);
            }
        };
        let manifest = request_module_line(b"E")
            .await
            .and_then(|line| parse_manifest(&line))
            .unwrap_or_default();
        let pm = pm_flags().await;

        let device_locale = probe
            .device_locale
            .clone()
            .unwrap_or_else(|| "unknown".to_owned());
        let requested_locale = params
            .locale
            .clone()
            .unwrap_or_else(|| device_locale.clone());
        let locale_honored = params.locale.is_none()
            || locales_compatible(&requested_locale, &device_locale)
            || requested_locale == device_locale;

        let mut items = Vec::with_capacity(apps.len());
        let mut missing_manifest = 0_u32;
        let mut missing_pm = 0_u32;
        let mut dropped_disabled = 0_u32;
        let mut enumerated_disabled = 0_u32;

        for app in &apps {
            let paths: Vec<&str> = manifest
                .get(&app.pkg)
                .map(|files| files.iter().map(|file| file.path.as_str()).collect())
                .unwrap_or_default();
            let is_system = match classify_system(&paths) {
                Some(value) => value,
                None => {
                    missing_manifest += 1;
                    false
                }
            };
            if matches!(params.scope, PackageScope::User) && is_system {
                continue;
            }
            if matches!(params.scope, PackageScope::System) && !is_system {
                continue;
            }
            let known = pm.enabled.contains(&app.pkg) || pm.disabled.contains(&app.pkg);
            if !known {
                // pm 未列出该包（OEM 私有包或枚举竞态）：不猜 uid，enabled 按真处理并计入 warning。
                missing_pm += 1;
            }
            let enabled = !pm.disabled.contains(&app.pkg);
            let uid = pm.uids.get(&app.pkg).copied();
            if !enabled {
                enumerated_disabled += 1;
                if !params.include_disabled {
                    dropped_disabled += 1;
                    continue;
                }
            }

            let label_is_resolved = !app.label.trim().is_empty() && app.label != app.pkg;
            let (label_source, fallback_reason) = if label_is_resolved {
                (LabelSource::Framework, None)
            } else {
                (
                    LabelSource::PackageName,
                    Some("framework_label_equals_package_name".to_owned()),
                )
            };
            let mut fallback_reason = fallback_reason;
            if !locale_honored {
                let reason = "requested_locale_not_applied_device_default".to_owned();
                fallback_reason = Some(match fallback_reason {
                    Some(existing) => format!("{existing};{reason}"),
                    None => reason,
                });
            }

            items.push(LocalizedPackageItem {
                package_name: app.pkg.clone(),
                label: if app.label.trim().is_empty() {
                    app.pkg.clone()
                } else {
                    app.label.clone()
                },
                version_name: Some(app.version_name.clone()).filter(|value| !value.is_empty()),
                version_code: app.version_code.and_then(|value| u64::try_from(value).ok()),
                requested_locale: requested_locale.clone(),
                resolved_locale: Some(device_locale.clone()),
                label_source,
                fallback_reason,
                uid,
                is_system,
                enabled,
            });
        }

        items.sort_by(|left, right| {
            (&left.label, &left.package_name).cmp(&(&right.label, &right.package_name))
        });

        let mut warnings = Vec::new();
        if !locale_honored {
            warnings.push(PackageWarning {
                package_name: None,
                code: "locale_not_honored".into(),
                message: format!(
                    "模块按设备默认 locale（{device_locale}）解析 label，请求的 {requested_locale} 未生效；需要指定 locale 时必须扩展模块侧 Resources 上下文"
                ),
            });
        }
        if dropped_disabled > 0 {
            warnings.push(PackageWarning {
                package_name: None,
                code: "disabled_excluded".into(),
                message: format!("按 include_disabled=false 过滤掉 {dropped_disabled} 个停用应用"),
            });
        }
        if params.include_disabled && enumerated_disabled == 0 {
            warnings.push(PackageWarning {
                package_name: None,
                code: "disabled_labels_unavailable".into(),
                message: "模块 Q 枚举不含停用应用，停用清单只有 pm 侧元数据、无本地化名称".into(),
            });
        }
        if missing_manifest > 0 {
            warnings.push(PackageWarning {
                package_name: None,
                code: "manifest_missing".into(),
                message: format!("{missing_manifest} 个包缺少 E 清单，is_system 按非系统包处理"),
            });
        }
        if missing_pm > 0 {
            warnings.push(PackageWarning {
                package_name: None,
                code: "pm_metadata_missing".into(),
                message: format!("{missing_pm} 个包在 pm 结果中缺失，uid/enabled 无法确认"),
            });
        }

        let fallback_count = u32::try_from(
            items
                .iter()
                .filter(|item| item.label_source != LabelSource::Framework)
                .count(),
        )
        .unwrap_or(u32::MAX);
        let success_count = items.len() as u32 - fallback_count;
        serialize(&PackageListLocalizedResult {
            items,
            success_count,
            fallback_count,
            warnings,
            channel: Some("zygisk_v1".to_owned()),
        })
    }

    /// v2：一次 `L` 就拿到带来源标注的完整条目，无需再跑 E/pm 拼接。
    async fn handle_list_pro(
        &self,
        params: &PackageListLocalizedParams,
        token: &str,
        probe: &Probe,
    ) -> Result<Value, AgentError> {
        let locale_arg = params.locale.clone().unwrap_or_else(|| "-".to_owned());
        let scope = match params.scope {
            PackageScope::All => "all",
            PackageScope::User => "user",
            PackageScope::System => "system",
        };
        let request = format!(
            "L {locale_arg} {scope} {}\n",
            u8::from(params.include_disabled)
        );
        let frames =
            pro_command_frames_with_token(token, &request, CAP_LIST, PACKAGE_LIST_LOCALIZED)
                .await
                .inspect_err(|error| self.mark_faulted(error))?;
        let final_frame = frames
            .iter()
            .rev()
            .find(|value| value.get("final").and_then(serde_json::Value::as_bool) == Some(true));
        let device_locale = probe
            .pro_locale
            .clone()
            .or_else(|| probe.device_locale.clone())
            .unwrap_or_else(|| String::from("unknown"));
        let requested_locale = params
            .locale
            .clone()
            .unwrap_or_else(|| device_locale.clone());

        let (mut items, unproven_locale, mut warnings) = map_pro_items(&frames, &requested_locale)?;
        items.sort_by(|left, right| {
            (&left.label, &left.package_name).cmp(&(&right.label, &right.package_name))
        });

        let fallback_count = u32::try_from(
            items
                .iter()
                .filter(|item| item.label_source != LabelSource::Framework)
                .count(),
        )
        .unwrap_or(u32::MAX);
        if let Some(final_frame) = final_frame {
            let reported = final_frame.get("count").and_then(serde_json::Value::as_u64);
            if reported.is_some_and(|count| usize::try_from(count) != Ok(items.len())) {
                warnings.push(PackageWarning {
                    package_name: None,
                    code: "count_mismatch".into(),
                    message: format!(
                        "模块自报 {} 条，实际解析 {} 条",
                        reported.unwrap_or_default(),
                        items.len()
                    ),
                });
            }
        }
        if unproven_locale > 0 {
            warnings.push(PackageWarning {
                package_name: None,
                code: "locale_unproven".into(),
                message: format!(
                    "{unproven_locale} 个应用无法确认是否按 {requested_locale} 命中资源                     （Android 不导出资源匹配结果），已按 labelSource/resolvedLocale 如实标注"
                ),
            });
        }
        if params.include_disabled {
            warnings.push(PackageWarning {
                package_name: None,
                code: "disabled_included".into(),
                message: "清单含停用应用，条目以 enabled=false 标注".into(),
            });
        }
        serialize(&PackageListLocalizedResult {
            success_count: items.len() as u32 - fallback_count,
            fallback_count,
            items,
            warnings,
            channel: Some("zygisk_v2".to_owned()),
        })
    }

    async fn handle_export(&self, params: Value) -> Result<Value, AgentError> {
        let params: PackageExportApkParams = parse_params(params)?;
        validate_package_name(&params.package_name)?;
        let probe = self.probe().await;
        require_bridge(&probe)?;

        let session = random_session().await?;
        let staging_dir = format!("{STAGING_ROOT}-{session}");
        create_private_dir(&staging_dir).await?;

        let staged = match probe.variant {
            Variant::Pro => {
                let token = self.pro_token().await.ok_or_else(|| {
                    provider_unavailable(
                        "v2 模块令牌不可用（Agent 需要 root 才能读令牌）",
                        Some("pro_token_missing"),
                    )
                })?;
                stage_pro_export(&token, &params.package_name, &staging_dir).await
            }
            _ => {
                let mut stream = connect_bridge_stream(CONNECT_TIMEOUT).await?;
                let result =
                    stage_package_export(&mut stream, &params.package_name, &staging_dir).await;
                drop(stream);
                result
            }
        };
        let (files, bytes, skipped) = match staged {
            Ok(value) => value,
            Err(error) => {
                // 失败也要回收设备侧副本；清理结果不掩盖原始错误。
                let _cleanup = tokio::fs::remove_dir_all(&staging_dir).await;
                return Err(error);
            }
        };

        if files.is_empty() {
            let _cleanup = tokio::fs::remove_dir_all(&staging_dir).await;
            return Err(AgentError::new(
                ErrorCode::NotFound,
                format!("模块未返回 {}/ 的任何 APK 文件", params.package_name),
            ));
        }
        serialize(&PackageExportApkResult {
            package_name: params.package_name,
            session,
            files,
            bytes,
            skipped,
        })
    }

    async fn handle_export_clean(&self, params: Value) -> Result<Value, AgentError> {
        let params: PackageExportCleanParams = parse_params(params)?;
        validate_session(&params.session)?;
        let staging_dir = format!("{STAGING_ROOT}-{}", params.session);
        let removed = match tokio::fs::remove_dir_all(&staging_dir).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(internal(format!("清理导出暂存目录失败: {error}"))),
        };
        serialize(&PackageExportCleanResult { removed })
    }

    fn mark_faulted(&self, error: &AgentError) {
        if let Some(mut probe) = self.cached() {
            probe.lifecycle = ZygiskLifecycle::Faulted;
            probe.at = Instant::now();
            probe.detail = Some(error.message.clone());
            self.store(probe);
        }
    }
}

impl Provider for ZygiskProvider {
    fn info(&self) -> ProviderInfo {
        let (health, last_error) = match self.cached() {
            None => (
                ProviderHealth::Unavailable,
                Some("zygisk bridge 尚未探测".to_owned()),
            ),
            Some(probe) => match probe.lifecycle {
                ZygiskLifecycle::BridgeReady => (ProviderHealth::Ready, None),
                ZygiskLifecycle::Loaded | ZygiskLifecycle::InstalledRebootRequired => (
                    ProviderHealth::Degraded,
                    probe.detail.clone().or_else(|| {
                        Some("模块已安装但 bridge 未就绪（通常需要重启或启用 Zygisk）".to_owned())
                    }),
                ),
                ZygiskLifecycle::Incompatible => (
                    ProviderHealth::Incompatible,
                    probe
                        .detail
                        .clone()
                        .or_else(|| Some("模块子协议不兼容".to_owned())),
                ),
                ZygiskLifecycle::Faulted => (
                    ProviderHealth::Faulted,
                    probe
                        .detail
                        .clone()
                        .or_else(|| Some("模块请求失败".to_owned())),
                ),
                ZygiskLifecycle::NotInstalled | ZygiskLifecycle::ZygiskDisabled => (
                    ProviderHealth::Unavailable,
                    probe
                        .detail
                        .clone()
                        .or_else(|| Some("Zygisk 模块未安装或未启用".to_owned())),
                ),
            },
        };
        ProviderInfo {
            name: "zygisk".into(),
            version: {
                let variant = self
                    .cached()
                    .map(|probe| probe.variant)
                    .unwrap_or(Variant::Absent);
                format!("{}-sub-{}", variant.module_id(), variant.sub_protocol())
            },
            health,
            required_permissions: vec!["zygisk_module".into()],
            last_error,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        ZYGISK_METHODS
    }

    /// 缓存空 = 后台预热还没出结果 = 未知。此时不该由桌面端替我们判定。
    fn probe_pending(&self) -> bool {
        self.cached().is_none()
    }

    fn unavailable_reason(&self, method: &str) -> Option<String> {
        // 生命周期诊断本身永远可用，否则 UI 拿不到「为什么不可用」。
        if method == ZYGISK_STATUS {
            return None;
        }
        match self.cached() {
            None => Some("zygisk bridge 尚未探测完成".to_owned()),
            Some(probe) if probe.bridge_ready => None,
            Some(probe) => Some(probe.detail.clone().unwrap_or_else(|| {
                format!(
                    "Zygisk 模块状态为 {:?}（通道 {:?}）",
                    probe.lifecycle, probe.variant
                )
            })),
        }
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                ZYGISK_STATUS => self.handle_status(params).await,
                PACKAGE_LIST_LOCALIZED => self.handle_list(params).await,
                PACKAGE_EXPORT_APK => self.handle_export(params).await,
                PACKAGE_EXPORT_CLEAN => self.handle_export_clean(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported zygisk method: {method}"),
                )),
            }
        })
    }
}

// ===== 模块线协议 =====

/// 模块 Q 报文按 camelCase 输出（`versionName` / `versionCode`），必须显式对齐，
/// 否则字段会静默取默认值——真机 versionName 就是这样被测试抓出来的。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModuleApp {
    pkg: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    version_name: String,
    #[serde(default)]
    version_code: Option<i64>,
}

/// E 清单条目：Provider 只消费路径（判系统/用户包），其余字段由导出协议自己校验。
#[derive(Debug, Deserialize)]
struct ModuleApk {
    #[serde(default)]
    path: String,
}

async fn bridge_alive(port: u16, timeout: Duration) -> bool {
    matches!(connect_port(port, timeout).await, Ok(_stream))
}

async fn connect_bridge_stream(timeout: Duration) -> Result<TcpStream, AgentError> {
    connect_port(MODULE_PORT, timeout).await
}

/// 连接指定模块端口（v1=11500 / v2=11501）。
async fn connect_port(port: u16, timeout: Duration) -> Result<TcpStream, AgentError> {
    let address = format!("{MODULE_HOST}:{port}");
    with_timeout(timeout, TcpStream::connect(&address))
        .await
        .map_err(|_| {
            provider_unavailable(
                format!("连接 Zygisk 模块 bridge 超时（{address}）"),
                Some("bridge_connect_timeout"),
            )
        })?
        .map_err(|error| {
            provider_unavailable(
                format!("无法连接 Zygisk 模块 bridge（{address}）: {error}"),
                Some("bridge_connect_failed"),
            )
        })
}

/// v2 导出：握手后 `E <pkg>`，按 `F/T/ERR/DONE` 行协议流式写入暂存目录。
async fn stage_pro_export(
    token: &str,
    package_name: &str,
    staging_dir: &str,
) -> Result<(Vec<StagedApkFile>, u64, Vec<String>), AgentError> {
    let mut stream = connect_port(PRO_PORT, CONNECT_TIMEOUT).await?;
    let mut reader = LineReader::new(&mut stream);
    let hello = format!("H {PRO_SUB_PROTOCOL_VERSION} {token}\n");
    reader.write_line(&hello, LINE_TIMEOUT).await?;
    let hello_line = reader
        .next_line(LINE_TIMEOUT)
        .await?
        .ok_or_else(|| provider_unavailable("v2 导出握手无应答", Some("pro_handshake_eof")))?;
    let hello = parse_pro_hello(&String::from_utf8_lossy(&hello_line))?;
    ensure_capability(&hello, CAP_EXPORT, PACKAGE_EXPORT_APK)?;

    let request = format!("E {package_name}\n");
    reader.write_line(&request, LINE_TIMEOUT).await?;

    let mut files = Vec::new();
    let mut total = 0_u64;
    let mut skipped = Vec::new();
    loop {
        let Some(line) = reader.next_line(LINE_TIMEOUT).await? else {
            return Err(provider_unavailable(
                "v2 导出连接提前结束",
                Some("export_truncated"),
            ));
        };
        if line == b"DONE".as_slice() {
            if files.is_empty() {
                return Err(AgentError::new(
                    ErrorCode::NotFound,
                    format!(
                        "模块未返回 {package_name} 的任何 APK 文件（跳过: {}）",
                        skipped.join(",")
                    ),
                ));
            }
            return Ok((files, total, skipped));
        }
        if let Some(text) = line
            .get(..4)
            .filter(|head| *head == b"ERR ")
            .map(|_| String::from_utf8_lossy(&line).to_string())
        {
            let (code, message) = parse_pro_error(text.trim_end())
                .unwrap_or_else(|| ("helper_failed".to_owned(), String::new()));
            return Err(pro_error_to_agent(&code, &message));
        }
        if line.first() == Some(&b'T') {
            // T <size> <pkg> <name> too_large：显式记录，不静默丢文件。
            // 只取文件名上报，宿主侧要能直接指出“产物里少了哪个分包”。
            let text = String::from_utf8_lossy(&line).into_owned();
            let name = text
                .split_whitespace()
                .nth(3)
                .filter(|value| !value.is_empty())
                .unwrap_or(text.trim())
                .to_owned();
            skipped.push(name);
            continue;
        }
        if line.first() != Some(&b'F') {
            continue;
        }
        let (size, pkg, name) = parse_export_header(&line)?;
        if pkg != package_name {
            return Err(incompatible(
                "v2 模块导出了非请求包的文件",
                Some("export_pkg_mismatch"),
            ));
        }
        validate_file_name(&name)?;
        if size > MAX_APK_BYTES || total.saturating_add(size) > MAX_EXPORT_BYTES {
            return Err(internal("导出 APK 超过体积上限"));
        }
        let remote_path = format!("{staging_dir}/{name}");
        let mut file = tokio::fs::File::create(&remote_path)
            .await
            .map_err(|error| internal(format!("创建暂存文件失败: {error}")))?;
        let copied = reader.read_exact_into(&mut file, size, CHUNK_TIMEOUT).await;
        drop(file);
        if let Err(error) = copied {
            let _cleanup = tokio::fs::remove_file(&remote_path).await;
            return Err(error);
        }
        total = total.saturating_add(size);
        files.push(StagedApkFile {
            name,
            size,
            remote_path,
        });
    }
}

async fn request_module_line(payload: &[u8]) -> Result<Vec<u8>, AgentError> {
    let mut stream = connect_bridge_stream(CONNECT_TIMEOUT).await?;
    with_timeout(LINE_TIMEOUT, stream.write_all(payload))
        .await
        .map_err(|_| deadline("模块请求写入超时"))?
        .map_err(|error| internal(format!("发送模块请求失败: {error}")))?;
    let mut reader = LineReader::new(&mut stream);
    reader.next_line(LINE_TIMEOUT).await?.ok_or_else(|| {
        provider_unavailable(
            "模块未返回数据（连接提前结束）",
            Some("bridge_empty_response"),
        )
    })
}

/// 读模块 `D` 流并写入 0700 暂存目录：包名、文件名、单文件与总长度全部校验。
/// 返回（文件列表, 总字节数, 被跳过的分片）；失败清理由调用方负责。
/// v1 demo 协议没有“跳过”这一说，恒定返回空列表。
async fn stage_package_export(
    stream: &mut TcpStream,
    package_name: &str,
    staging_dir: &str,
) -> Result<(Vec<StagedApkFile>, u64, Vec<String>), AgentError> {
    let mut payload = Vec::with_capacity(package_name.len() + 4);
    payload.extend_from_slice(b"D");
    payload.extend_from_slice(package_name.as_bytes());
    payload.extend_from_slice(b"\n\n");
    with_timeout(CHUNK_TIMEOUT, stream.write_all(&payload))
        .await
        .map_err(|_| deadline("模块导出请求超时"))?
        .map_err(|error| internal(format!("发送导出请求失败: {error}")))?;

    let mut reader = LineReader::new(stream);
    let mut files = Vec::new();
    let mut total = 0_u64;
    loop {
        let header = match reader.next_line(LINE_TIMEOUT).await {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Err(provider_unavailable(
                    "模块导出连接提前结束",
                    Some("export_truncated"),
                ));
            }
            Err(error) => return Err(error),
        };
        if header == b"DONE".as_slice() {
            return Ok((files, total, Vec::new()));
        }
        if header.starts_with(b"ERR") {
            return Err(internal(format!(
                "模块导出失败: {}",
                String::from_utf8_lossy(&header)
            )));
        }
        if header.is_empty() {
            continue;
        }
        let (size, pkg, name) = parse_export_header(&header)?;
        if pkg != package_name {
            return Err(incompatible(
                "模块导出了非请求包的文件",
                Some("export_pkg_mismatch"),
            ));
        }
        validate_file_name(&name)?;
        if size > MAX_APK_BYTES || total.saturating_add(size) > MAX_EXPORT_BYTES {
            return Err(internal("导出 APK 超过体积上限"));
        }
        let remote_path = format!("{staging_dir}/{name}");
        let mut file = tokio::fs::File::create(&remote_path)
            .await
            .map_err(|error| internal(format!("创建暂存文件失败: {error}")))?;
        let copied = reader.read_exact_into(&mut file, size, CHUNK_TIMEOUT).await;
        drop(file);
        if let Err(error) = copied {
            let _cleanup = tokio::fs::remove_file(&remote_path).await;
            return Err(error);
        }
        total = total.saturating_add(size);
        files.push(StagedApkFile {
            name,
            size,
            remote_path,
        });
    }
}

// ===== v2（applistpro）客户端：令牌握手 + 长度前缀分帧 =====

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProHello {
    version: String,
    version_code: u32,
    locale: String,
    capabilities: Vec<String>,
    /// 不来自握手行：模块宣告 `handlers` 能力后，由 `I` 单独查回来（AR10.2）。
    handlers: Vec<ModuleHandlerInfo>,
}

/// Agent↔模块私有子协议的兼容规则（AR10.1 固化，实现细节见
/// `android-zygisk/INTEGRATION.md`）。这四条不是愿望，而是**下面代码实际执行**的行为：
///
/// 1. **主版本号严格匹配**：`OK` 第二字段不等于 `PRO_SUB_PROTOCOL_VERSION` 立即判
///    `incompatible`，不做"看起来差不多就试试"的猜测——线格式变化就是变化。
///    增量能力一律走能力位（下一条），不靠版本号猜。
/// 2. **能力位是唯一的增量通道**：握手行的尾部是空格分隔的能力名。
///    新模块宣告老 Desktop 不认识的能力 → 老 Desktop 原样忽略，握手照常成功；
///    老模块没宣告某个能力 → 对应 method 在**发出命令之前**就被拒
///    （`unsupported_method` + `reason=capability_missing`），不是等到超时再猜。
/// 3. **旧 Desktop + 新模块**必须可用：Agent 只按自己认识的能力名取值，
///    因此新增能力对旧端是纯加法；破坏性改动才允许升版本号。
/// 4. **v2 不可用不等于 Zygisk 不可用**：v2 探测失败时按既有顺序退回 v1 demo，
///    并在 `zygisk.status` 里如实说明是哪一档（AR5.3 契约），Shell 能等价实现的
///    普通能力仍按路由规则降级。
///
/// 握手应答行的线格式：`H 2 <token>` -> `OK 2 <version> <versionCode> <deviceLocale> <caps...>`。
fn parse_pro_hello(line: &str) -> Result<ProHello, AgentError> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("OK") {
        return Err(incompatible(
            format!("v2 模块握手应答异常: {line}"),
            Some("pro_handshake_shape"),
        ));
    }
    let proto: u32 = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| incompatible("v2 握手缺少协议版本", Some("pro_handshake_proto")))?;
    if proto != PRO_SUB_PROTOCOL_VERSION {
        return Err(incompatible(
            format!("v2 模块协议版本不匹配: {proto}"),
            Some("pro_handshake_version"),
        ));
    }
    let version = parts.next().unwrap_or_default().to_owned();
    let version_code = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    // 设备 locale 可能整体含空格？协议里它是单个 BCP-47 标签，安全按字段取
    let locale = parts.next().unwrap_or_default().to_owned();
    let capabilities = parts.map(str::to_owned).collect();
    Ok(ProHello {
        version,
        version_code,
        locale,
        capabilities,
        handlers: Vec::new(),
    })
}

/// `ERR <code>[ <消息>]`：无消息时模块侧会留一个尾随空格，必须按空白切分而不是整串比较。
fn parse_pro_error(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("ERR ")?;
    let mut parts = rest.splitn(2, ' ');
    let code = parts.next()?.to_owned();
    let message = parts.next().unwrap_or_default().trim().to_owned();
    Some((code, message))
}

fn pro_error_to_agent(code: &str, message: &str) -> AgentError {
    let detail = if message.is_empty() {
        format!("v2 模块返回 {code}")
    } else {
        format!("v2 模块返回 {code}: {message}")
    };
    match code {
        "auth_failed" | "auth_required" => AgentError::new(ErrorCode::PermissionDenied, detail),
        "unsupported_protocol" => incompatible(detail, Some("pro_protocol_mismatch")),
        "no_files" => AgentError::new(ErrorCode::NotFound, detail),
        "bad_package" | "bad_scope" | "bad_locale" | "bad_request" | "bad_handshake" => {
            AgentError::new(ErrorCode::InvalidRequest, detail)
        }
        _ => internal(detail),
    }
}

async fn handshake_pro(token: &str) -> Result<ProHello, AgentError> {
    let mut stream = connect_port(PRO_PORT, CONNECT_TIMEOUT).await?;
    let request = format!("H {PRO_SUB_PROTOCOL_VERSION} {token}\n");
    with_timeout(LINE_TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| deadline("v2 握手写入超时"))?
        .map_err(|error| internal(format!("v2 握手写入失败: {error}")))?;
    let mut reader = LineReader::new(&mut stream);
    let line = reader
        .next_line(PROBE_IO_TIMEOUT)
        .await?
        .ok_or_else(|| provider_unavailable("v2 模块握手无应答", Some("pro_handshake_eof")))?;
    // 不回显令牌：错误信息里只带模块返回的状态行
    parse_pro_hello(&String::from_utf8_lossy(&line))
}

/// 问模块要它自己的 handler 注册表（`I`）。帧内容是每行一个 JSON 对象。
async fn fetch_module_handlers(token: &str) -> Result<Vec<ModuleHandlerInfo>, AgentError> {
    let frames = pro_command_frames_with_token(token, "I\n", CAP_HANDLERS, "zygisk.status").await?;
    Ok(parse_handler_frames(&frames))
}

/// 纯函数：从分帧结果里挑出结构完整的 handler。认不出的字段忽略、
/// 单条坏掉不影响其它条——模块侧加了新字段不能反过来把旧 Agent 打挂。
fn parse_handler_frames(frames: &[serde_json::Value]) -> Vec<ModuleHandlerInfo> {
    let mut out = Vec::new();
    for frame in frames {
        if frame.get("final").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        if let Ok(info) = serde_json::from_value::<ModuleHandlerInfo>(frame.clone()) {
            if !info.cmd.is_empty() {
                out.push(info);
            }
        }
    }
    out
}

/// 握手能力位门控（AR10.1 规则 2）。`capability` 传空串表示该命令不需要能力
/// （例如 `S` 状态查询）。缺能力必须在**发命令之前**失败：让模块去猜一个它不认识的
/// 命令，只会把「能力缺失」变成一次超时或一个莫名其妙的 `ERR unsupported_command`。
fn ensure_capability(hello: &ProHello, capability: &str, method: &str) -> Result<(), AgentError> {
    if capability.is_empty() || hello.capabilities.iter().any(|cap| cap == capability) {
        return Ok(());
    }
    Err(AgentError::new(
        ErrorCode::UnsupportedMethod,
        format!(
            "模块 {module} 未宣告 `{capability}` 能力，{method} 不可用（模块版本 {}）",
            hello.version,
            module = PRO_MODULE_ID,
        ),
    )
    .with_details(serde_json::json!({
        "reason": "capability_missing",
        "capability": capability,
        "method": method,
        "module_version": hello.version,
        "advertised": hello.capabilities,
    })))
}

/// 发一条 v2 命令并读完分帧响应（末帧 `{"final":true}`）；ERR 行转成结构化错误。
/// 令牌必须由 provider 显式传入：它不进日志、不进错误信息。
async fn pro_command_frames_with_token(
    token: &str,
    request: &str,
    capability: &str,
    method: &str,
) -> Result<Vec<serde_json::Value>, AgentError> {
    let mut stream = connect_port(PRO_PORT, CONNECT_TIMEOUT).await?;
    let mut reader = LineReader::new(&mut stream);
    let hello_request = format!("H {PRO_SUB_PROTOCOL_VERSION} {token}\n");
    reader.write_line(&hello_request, LINE_TIMEOUT).await?;
    let hello = reader
        .next_line(LINE_TIMEOUT)
        .await?
        .ok_or_else(|| provider_unavailable("v2 模块握手无应答", Some("pro_handshake_eof")))?;
    let hello = parse_pro_hello(&String::from_utf8_lossy(&hello))?;
    ensure_capability(&hello, capability, method)?;

    reader.write_line(request, LINE_TIMEOUT).await?;

    let mut frames = Vec::new();
    loop {
        let mut head = [0_u8; 4];
        with_timeout(LINE_TIMEOUT, stream.read_exact(&mut head))
            .await
            .map_err(|_| deadline("等待 v2 响应帧头超时"))?
            .map_err(|error| {
                provider_unavailable(format!("读取 v2 帧头失败: {error}"), Some("pro_frame_head"))
            })?;
        let len = u32::from_be_bytes(head) as usize;
        if len > MAX_LINE_BYTES as usize {
            return Err(internal("v2 响应单帧超过大小上限"));
        }
        let mut body = vec![0_u8; len];
        with_timeout(LINE_TIMEOUT, stream.read_exact(&mut body))
            .await
            .map_err(|_| deadline("等待 v2 响应帧体超时"))?
            .map_err(|error| internal(format!("读取 v2 帧体失败: {error}")))?;
        let text = String::from_utf8_lossy(&body).to_string();
        if let Some((code, message)) = parse_pro_error(text.trim_end()) {
            return Err(pro_error_to_agent(&code, &message));
        }
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            incompatible(
                format!("v2 响应帧不是合法 JSON: {error}"),
                Some("pro_frame_json"),
            )
        })?;
        let final_frame = value.get("final").and_then(serde_json::Value::as_bool) == Some(true);
        frames.push(value);
        if final_frame {
            return Ok(frames);
        }
    }
}

/// 把 v2 清单帧转成公共条目：保留模块给出的 labelSource / resolvedLocale / fallbackReason，
/// 并统计「无法证明按请求 locale 命中」的条目数（Android 不导出资源匹配结果）。
fn map_pro_items(
    frames: &[serde_json::Value],
    requested_locale: &str,
) -> Result<(Vec<LocalizedPackageItem>, u32, Vec<PackageWarning>), AgentError> {
    let mut items = Vec::with_capacity(frames.len());
    let mut unproven_locale = 0_u32;
    let mut warnings = Vec::new();
    for value in frames
        .iter()
        .filter(|value| value.get("final").and_then(serde_json::Value::as_bool) != Some(true))
    {
        // 单条字段异常只影响该条：记 item warning 后继续，保住整批清单（AR5.4 契约）
        let item: ProItem = match serde_json::from_value(value.clone()) {
            Ok(item) => item,
            Err(error) => {
                let pkg = value
                    .get("pkg")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                warnings.push(PackageWarning {
                    package_name: pkg.clone(),
                    code: "item_parse_failed".into(),
                    message: format!(
                        "{} 条目字段异常，已从清单跳过: {error}",
                        pkg.unwrap_or_else(|| "<未知包名>".to_owned())
                    ),
                });
                continue;
            }
        };
        let label_source = match item.label_source.as_str() {
            "framework" => LabelSource::Framework,
            "manifest" => LabelSource::Manifest,
            _ => LabelSource::PackageName,
        };
        if item
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("locale_"))
            || label_source == LabelSource::PackageName
        {
            unproven_locale += 1;
        }
        items.push(LocalizedPackageItem {
            package_name: item.pkg,
            label: item.label,
            version_name: Some(item.version_name).filter(|value| !value.is_empty()),
            version_code: item
                .version_code
                .and_then(|value| u64::try_from(value).ok()),
            requested_locale: requested_locale.to_owned(),
            resolved_locale: item.resolved_locale,
            label_source,
            fallback_reason: item.fallback_reason,
            uid: item.uid,
            is_system: item.is_system,
            enabled: item.enabled,
        });
    }
    Ok((items, unproven_locale, warnings))
}

fn parse_apps(bytes: &[u8]) -> Result<Vec<ModuleApp>, AgentError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        incompatible(
            format!("模块应用清单 JSON 无法解析: {error}"),
            Some("apps_json_invalid"),
        )
    })?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(internal(format!("模块应用清单失败: {error}")));
    }
    let apps: Vec<ModuleApp> = serde_json::from_value(value).map_err(|error| {
        incompatible(
            format!("模块应用清单字段不兼容: {error}"),
            Some("apps_schema_incompatible"),
        )
    })?;
    Ok(apps)
}

fn parse_manifest(bytes: &[u8]) -> Result<BTreeMap<String, Vec<ModuleApk>>, AgentError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        incompatible(
            format!("模块 APK 清单 JSON 无法解析: {error}"),
            Some("manifest_json_invalid"),
        )
    })?;
    if value.get("error").is_some() {
        return Ok(BTreeMap::new());
    }
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn parse_export_header(line: &[u8]) -> Result<(u64, String, String), AgentError> {
    let text = std::str::from_utf8(line)
        .map_err(|_| incompatible("模块导出文件头不是 UTF-8", Some("export_header_utf8")))?;
    let mut parts = text.splitn(4, ' ');
    if parts.next() != Some("F") {
        return Err(incompatible(
            format!("模块导出未知文件头: {text}"),
            Some("export_header_shape"),
        ));
    }
    let size = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| incompatible("模块导出文件头缺少合法大小", Some("export_header_size")))?;
    let package_name = parts
        .next()
        .ok_or_else(|| incompatible("模块导出文件头缺少包名", Some("export_header_pkg")))?
        .to_owned();
    let name = parts
        .next()
        .ok_or_else(|| incompatible("模块导出文件头缺少文件名", Some("export_header_name")))?
        .to_owned();
    Ok((size, package_name, name))
}

struct LineReader<'a> {
    stream: &'a mut TcpStream,
}

impl LineReader<'_> {
    async fn write_line(&mut self, text: &str, timeout: Duration) -> Result<(), AgentError> {
        with_timeout(timeout, self.stream.write_all(text.as_bytes()))
            .await
            .map_err(|_| deadline("写入模块请求超时"))?
            .map_err(|error| internal(format!("写入模块请求失败: {error}")))
    }
}

impl<'a> LineReader<'a> {
    fn new(stream: &'a mut TcpStream) -> Self {
        Self { stream }
    }

    async fn next_line(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, AgentError> {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            if line.len() as u64 > MAX_LINE_BYTES {
                return Err(internal("模块响应单行超过大小上限"));
            }
            let read = with_timeout(timeout, self.stream.read(&mut byte))
                .await
                .map_err(|_| deadline("等待模块响应超时"))?
                .map_err(|error| internal(format!("读取模块响应失败: {error}")))?;
            if read == 0 {
                return Ok(if line.is_empty() { None } else { Some(line) });
            }
            if byte[0] == b'\n' {
                return Ok(Some(line));
            }
            line.push(byte[0]);
        }
    }

    async fn read_exact_into(
        &mut self,
        file: &mut tokio::fs::File,
        size: u64,
        timeout: Duration,
    ) -> Result<(), AgentError> {
        let mut remaining = size;
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining > 0 {
            let want = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            let read = with_timeout(timeout, self.stream.read(&mut buffer[..want]))
                .await
                .map_err(|_| deadline("接收 APK 分块超时"))?
                .map_err(|error| internal(format!("接收 APK 数据失败: {error}")))?;
            if read == 0 {
                return Err(provider_unavailable(
                    "APK 传输提前结束",
                    Some("export_truncated"),
                ));
            }
            with_timeout(timeout, file.write_all(&buffer[..read]))
                .await
                .map_err(|_| deadline("写入暂存文件超时"))?
                .map_err(|error| internal(format!("写入暂存文件失败: {error}")))?;
            remaining -= read as u64;
        }
        Ok(())
    }
}

async fn with_timeout<T>(
    timeout: Duration,
    future: impl std::future::Future<Output = T>,
) -> Result<T, ()> {
    match tokio::time::timeout(timeout, future).await {
        Ok(value) => Ok(value),
        Err(_) => Err(()),
    }
}

// ===== 设备侧探测 =====

/// 单条固定脚本，无任何外部输入拼接；root 不可用时返回 None（不猜测模块状态）。
// 一次固定脚本探测两个模块（无任何外部输入拼接）；root 不可用时整块失败。
const ROOT_PROBE_SCRIPT: &str = concat!(
    "echo ROOT=1;",
    "for m in applistpro applist; do",
    " if [ -d \"/data/adb/modules/$m\" ]; then echo \"INSTALLED $m\"; fi;",
    " if [ -d \"/data/adb/modules_update/$m\" ]; then echo \"PENDING $m\"; fi;",
    " if [ -f \"/data/adb/modules/$m/disable\" ]; then echo \"DISABLED $m\"; fi;",
    " if [ -f \"/data/adb/modules/$m/remove\" ]; then echo \"REMOVING $m\"; fi;",
    " if [ -f \"/data/adb/modules/$m/module.prop\" ]; then echo \"PROP_BEGIN $m\";",
    " cat \"/data/adb/modules/$m/module.prop\"; echo \"PROP_END\"; fi;",
    " done;",
    "for d in /data/adb/zygisksu /data/adb/zygisk /data/adb/ap/zygisk /data/adb/kzygisk; do",
    " if [ -e \"$d\" ]; then echo \"ZYGIMPL=$(basename \"$d\")\"; fi; done;",
    "echo PROBE_DONE=1",
);

async fn probe_root() -> Option<RootFacts> {
    let output = with_timeout(
        ROOT_TIMEOUT,
        Command::new("su").args(["-c", ROOT_PROBE_SCRIPT]).output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(classify_root_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn classify_root_output(text: &str) -> RootFacts {
    let mut facts = RootFacts::default();
    let mut prop_owner: Option<&str> = None;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("PROP_BEGIN ") {
            prop_owner = Some(rest);
            continue;
        }
        if line == "PROP_END" {
            prop_owner = None;
            continue;
        }
        if let Some(owner) = prop_owner {
            if let Some((key, value)) = line.split_once('=') {
                let target = match owner {
                    "applistpro" => Some(&mut facts.pro),
                    "applist" => Some(&mut facts.demo),
                    _ => None,
                };
                if let Some(module) = target {
                    match key {
                        "version" => module.version = Some(value.to_owned()),
                        "versionCode" => module.version_code = value.parse().ok(),
                        _ => {}
                    }
                }
            }
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let marker = parts.next().unwrap_or("");
        let target = match marker {
            "INSTALLED" | "PENDING" | "DISABLED" | "REMOVING" => match parts.next() {
                Some("applistpro") => Some(&mut facts.pro),
                Some("applist") => Some(&mut facts.demo),
                _ => None,
            },
            _ => None,
        };
        match (marker, target) {
            ("INSTALLED", Some(module)) => module.installed = true,
            ("PENDING", Some(module)) => module.pending_update = true,
            ("DISABLED", Some(module)) => module.disabled_marker = true,
            ("REMOVING", Some(module)) => module.remove_marker = true,
            _ => {
                if let Some(value) = line.strip_prefix("ZYGIMPL=")
                    && facts.zygisk_impl.is_none()
                    && !value.is_empty()
                {
                    facts.zygisk_impl = Some(value.to_owned());
                }
            }
        }
    }
    facts
}
/// 生命周期判定：先由「实际探测到的通道」决定，再用 root 事实细化原因。
/// v2 可达但没令牌（ProLocked）不能冒充可用，报成 installed_reboot_required 之外
/// 最贴近的状态：模块已装但 Agent 无法鉴权 -> faulted + 明确 detail。
fn classify_lifecycle(
    variant: Variant,
    root: Option<&RootFacts>,
) -> (ZygiskLifecycle, Option<String>) {
    let facts = match root {
        None => {
            return match variant {
                Variant::Pro => (
                    ZygiskLifecycle::BridgeReady,
                    Some("root 不可用，模块安装状态未探测；v2 握手已成功".to_owned()),
                ),
                Variant::Demo => (
                    ZygiskLifecycle::BridgeReady,
                    Some("root 不可用，模块安装状态未探测；v1 bridge 已响应".to_owned()),
                ),
                Variant::ProLocked => (
                    ZygiskLifecycle::Faulted,
                    Some("v2 模块在监听但 Agent 读不到令牌（需要 root）".to_owned()),
                ),
                Variant::Absent => (
                    ZygiskLifecycle::Faulted,
                    Some(
                        "root 不可用且两个模块端口均未响应，无法区分未安装/未启用/需重启"
                            .to_owned(),
                    ),
                ),
            };
        }
        Some(facts) => facts,
    };
    let module = match variant.module_id() {
        PRO_MODULE_ID => &facts.pro,
        _ => &facts.demo,
    };
    match variant {
        Variant::Pro => {
            if facts.demo.pending_update || facts.pro.pending_update {
                return (
                    ZygiskLifecycle::InstalledRebootRequired,
                    Some("检测到模块待更新：当前应答的仍是重启前已加载的版本".to_owned()),
                );
            }
            (ZygiskLifecycle::BridgeReady, None)
        }
        Variant::Demo => {
            if facts.demo.pending_update {
                return (
                    ZygiskLifecycle::InstalledRebootRequired,
                    Some("demo 模块已安装/升级，需重启加载新的 zygisk .so".to_owned()),
                );
            }
            (
                ZygiskLifecycle::BridgeReady,
                Some("仅 v1 demo 模块可用（无鉴权、无法按指定 locale 解析）".to_owned()),
            )
        }
        Variant::ProLocked => (
            ZygiskLifecycle::Faulted,
            Some(format!(
                "/data/adb/modules/{PRO_MODULE_ID}/token 读不到：Agent 需要 root 才能取令牌，                 而 v1 demo 模块也不在监听"
            )),
        ),
        Variant::Absent => {
            if module.remove_marker {
                return (
                    ZygiskLifecycle::NotInstalled,
                    Some("模块已标记删除，将在下次启动移除".to_owned()),
                );
            }
            if !facts.pro.installed && !facts.demo.installed {
                return (
                    ZygiskLifecycle::NotInstalled,
                    Some("applistpro/applist 均未安装，需要推送并安装模块 ZIP".to_owned()),
                );
            }
            if facts.pro.disabled_marker
                || facts.demo.disabled_marker
                || facts.zygisk_impl.is_none()
            {
                return (
                    ZygiskLifecycle::ZygiskDisabled,
                    Some("模块或 Zygisk 实现被禁用（检查 KernelSU/Magisk 的 Zygisk 开关与模块 enable）".to_owned()),
                );
            }
            if facts.pro.pending_update || facts.demo.pending_update {
                return (
                    ZygiskLifecycle::InstalledRebootRequired,
                    Some("模块已安装/升级，需重启加载新的 zygisk .so".to_owned()),
                );
            }
            (
                ZygiskLifecycle::Loaded,
                Some(
                    "模块已安装但 bridge 未监听（system_server 侧握手未完成或模块异常退出）"
                        .to_owned(),
                ),
            )
        }
    }
}
async fn detect_device_locale() -> Option<String> {
    let property = run("/system/bin/getprop", &["persist.sys.locale"]).await?;
    let property = property.trim().to_owned();
    if !property.is_empty() {
        return Some(property);
    }
    let settings = run("/system/bin/settings", &["get", "system", "system_locales"]).await;
    if let Some(value) = settings {
        let first = value
            .trim()
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !first.is_empty() && first != "null" {
            return Some(first);
        }
    }
    let fallback = run("/system/bin/getprop", &["ro.product.locale"]).await?;
    let fallback = fallback.trim().to_owned();
    (!fallback.is_empty()).then_some(fallback)
}

async fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = with_timeout(ROOT_TIMEOUT, Command::new(program).args(args).output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// `pm list packages -e -U` / `-d -U`：设备端解析，Desktop 只见结构化结果。
#[derive(Debug, Default)]
struct PmFlags {
    uids: HashMap<String, u32>,
    enabled: HashSet<String>,
    disabled: HashSet<String>,
}

async fn pm_flags() -> PmFlags {
    let mut flags = PmFlags::default();
    if let Some(text) = run("/system/bin/pm", &["list", "packages", "-e", "-U"]).await {
        collect_pm_lines(&text, &mut flags, false);
    }
    if let Some(text) = run("/system/bin/pm", &["list", "packages", "-d", "-U"]).await {
        collect_pm_lines(&text, &mut flags, true);
    }
    flags
}

fn collect_pm_lines(text: &str, flags: &mut PmFlags, disabled: bool) {
    // 解析规则与 ShellProvider 的 package.list 共用一份，避免两处各自理解 `pm` 输出。
    for (package, uid) in super::device::parse_pm_uid_lines(text) {
        if disabled {
            flags.disabled.insert(package.clone());
        } else {
            flags.enabled.insert(package.clone());
        }
        if let Some(uid) = uid {
            flags.uids.entry(package).or_insert(uid);
        }
    }
}

fn classify_system(paths: &[&str]) -> Option<bool> {
    let first = paths.first()?;
    if first.starts_with("/data/app/") || first.starts_with("/data/user/") {
        Some(false)
    } else if first.starts_with('/') {
        Some(true)
    } else {
        None
    }
}

/// BCP-47 粗粒度比较：语言必须相同；请求未带地区即视为兼容；
/// 带地区则设备地区必须一致。脚本子标签（`zh-Hans-CN` 的 `hans`）不参与判定，
/// 否则 `zh-CN` 会被误判成与设备 `zh-Hans-CN` 不兼容。
fn locales_compatible(requested: &str, device: &str) -> bool {
    fn subtags(value: &str) -> Vec<String> {
        value
            .to_ascii_lowercase()
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }
    fn region(tags: &[String]) -> Option<String> {
        tags.iter().skip(1).find(|tag| tag.len() == 2).cloned()
    }

    let requested = subtags(requested);
    let device = subtags(device);
    let (Some(requested_language), Some(device_language)) = (requested.first(), device.first())
    else {
        return false;
    };
    if requested_language != device_language {
        return false;
    }
    match region(&requested) {
        Some(wanted) => region(&device).is_some_and(|actual| actual == wanted),
        None => true,
    }
}

fn require_bridge(probe: &Probe) -> Result<(), AgentError> {
    if probe.bridge_ready {
        return Ok(());
    }
    let detail = probe
        .detail
        .clone()
        .unwrap_or_else(|| format!("模块状态 {:?}", probe.lifecycle));
    Err(provider_unavailable(
        format!("Zygisk 模块 bridge 不可用：{detail}"),
        Some("zygisk_bridge_unavailable"),
    ))
}

fn provider_unavailable(message: impl Into<String>, code: Option<&str>) -> AgentError {
    with_code(ErrorCode::ProviderUnavailable, message, code)
}

fn incompatible(message: impl Into<String>, code: Option<&str>) -> AgentError {
    with_code(ErrorCode::IncompatibleVersion, message, code)
}

fn deadline(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::DeadlineExceeded, message)
}

fn internal(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Internal, message)
}

fn with_code(code: ErrorCode, message: impl Into<String>, reason: Option<&str>) -> AgentError {
    match reason {
        Some(reason) => {
            AgentError::new(code, message).with_details(serde_json::json!({ "reason": reason }))
        }
        None => AgentError::new(code, message),
    }
}

fn validate_package_name(package_name: &str) -> Result<(), AgentError> {
    let ok = !package_name.is_empty()
        && package_name.len() <= 256
        && package_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "包名不合法，拒绝导出请求",
        ))
    }
}

fn validate_file_name(name: &str) -> Result<(), AgentError> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(incompatible(
            format!("模块返回了不安全的文件名: {name}"),
            Some("export_name_unsafe"),
        ))
    }
}

fn validate_session(session: &str) -> Result<(), AgentError> {
    let ok = session.len() == 16
        && session
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if ok {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "导出会话标识格式非法",
        ))
    }
}

async fn create_private_dir(path: &str) -> Result<(), AgentError> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .await
        .map_err(|error| internal(format!("创建导出暂存目录失败: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| internal(format!("设置暂存目录权限失败: {error}")))?;
    }
    Ok(())
}

async fn random_session() -> Result<String, AgentError> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open("/dev/urandom")
        .await
        .map_err(|error| internal(format!("打开随机源失败: {error}")))?;
    let mut bytes = [0_u8; 8];
    file.read_exact(&mut bytes)
        .await
        .map_err(|error| internal(format!("读取随机源失败: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "zygisk provider 参数不合法")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize<T: serde::Serialize>(value: &T) -> Result<Value, AgentError> {
    to_value(value).map_err(|error| internal(format!("序列化 Provider 结果失败: {error}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    const ROOT_SAMPLE: &str = concat!(
        "ROOT=1\n",
        "INSTALLED applistpro\n",
        "PENDING applistpro\n",
        "INSTALLED applist\n",
        "PROP_BEGIN applistpro\n",
        "id=applistpro\nversion=v2.0\nversionCode=2\n",
        "PROP_END\n",
        "PROP_BEGIN applist\n",
        "id=applist\nversion=v1.0\nversionCode=1\n",
        "PROP_END\n",
        "ZYGIMPL=zygisksu\n",
        "PROBE_DONE=1\n",
    );

    #[test]
    fn root_probe_reads_both_modules_from_one_script() {
        let facts = classify_root_output(ROOT_SAMPLE);
        assert!(facts.pro.installed && facts.pro.pending_update);
        assert_eq!(facts.pro.version.as_deref(), Some("v2.0"));
        assert_eq!(facts.pro.version_code, Some(2));
        assert!(facts.demo.installed && !facts.demo.pending_update);
        assert_eq!(facts.demo.version.as_deref(), Some("v1.0"));
        assert_eq!(facts.demo.version_code, Some(1));
        assert_eq!(facts.zygisk_impl.as_deref(), Some("zygisksu"));
    }

    #[test]
    fn pending_update_on_either_channel_reports_reboot_required() {
        let facts = classify_root_output(ROOT_SAMPLE);
        // v2 端口活着但 modules_update 里还有新版本 -> 当前应答来自重启前的 .so
        let (lifecycle, detail) = classify_lifecycle(Variant::Pro, Some(&facts));
        assert_eq!(lifecycle, ZygiskLifecycle::InstalledRebootRequired);
        assert!(detail.unwrap().contains("重启前"));

        let mut clean = facts.clone();
        clean.pro.pending_update = false;
        assert_eq!(
            classify_lifecycle(Variant::Pro, Some(&clean)).0,
            ZygiskLifecycle::BridgeReady
        );
    }

    #[test]
    fn lifecycle_without_root_stays_honest() {
        let (ready, detail) = classify_lifecycle(Variant::Pro, None);
        assert_eq!(ready, ZygiskLifecycle::BridgeReady);
        assert!(detail.unwrap().contains("root 不可用"));

        let (locked, detail) = classify_lifecycle(Variant::ProLocked, None);
        assert_eq!(locked, ZygiskLifecycle::Faulted);
        assert!(detail.unwrap().contains("读不到令牌"));

        let (unknown, detail) = classify_lifecycle(Variant::Absent, None);
        assert_eq!(unknown, ZygiskLifecycle::Faulted);
        assert!(detail.unwrap().contains("无法区分"));
    }

    #[test]
    fn absent_channel_distinguishes_missing_disabled_and_loaded() {
        let mut facts = classify_root_output(ROOT_SAMPLE);
        facts.pro.installed = false;
        facts.demo.installed = false;
        assert_eq!(
            classify_lifecycle(Variant::Absent, Some(&facts)).0,
            ZygiskLifecycle::NotInstalled
        );

        facts.demo.installed = true;
        facts.demo.disabled_marker = true;
        assert_eq!(
            classify_lifecycle(Variant::Absent, Some(&facts)).0,
            ZygiskLifecycle::ZygiskDisabled
        );

        facts.demo.disabled_marker = false;
        facts.pro.pending_update = false;
        assert_eq!(
            classify_lifecycle(Variant::Absent, Some(&facts)).0,
            ZygiskLifecycle::Loaded
        );

        facts.zygisk_impl = None;
        assert_eq!(
            classify_lifecycle(Variant::Absent, Some(&facts)).0,
            ZygiskLifecycle::ZygiskDisabled
        );
    }

    #[test]
    fn channel_selection_falls_back_explicitly_not_silently() {
        let hello = ProHello {
            version: "v2.0".into(),
            version_code: 2,
            locale: "zh-Hans-CN".into(),
            capabilities: vec!["list".into()],
            handlers: Vec::new(),
        };
        // v2 握手成功：无论 v1 是否活着都用 v2
        for demo in [true, false] {
            let picked = select_variant(ProState::Ready(hello.clone()), demo);
            assert_eq!(picked.variant, Variant::Pro);
            assert_eq!(picked.pro_locale.as_deref(), Some("zh-Hans-CN"));
            assert!(picked.note.is_none());
        }
        // 读不到令牌 + v1 可用：退回 v1 但必须留原因
        let fallback = select_variant(ProState::NeedsToken, true);
        assert_eq!(fallback.variant, Variant::Demo);
        assert!(fallback.note.unwrap().contains("root"));
        // 读不到令牌 + v1 也没有：ProLocked，不猜「未安装」
        assert_eq!(
            select_variant(ProState::NeedsToken, false).variant,
            Variant::ProLocked
        );
        // 握手失败同样显式退回
        let broken = select_variant(ProState::HandshakeFailed("bad proto".into()), true);
        assert_eq!(broken.variant, Variant::Demo);
        assert!(broken.note.unwrap().contains("bad proto"));
        assert_eq!(
            select_variant(ProState::HandshakeFailed("bad proto".into()), false).variant,
            Variant::ProLocked
        );
        // v2 完全不在：有 v1 走 v1，否则 Absent
        assert_eq!(select_variant(ProState::Dead, true).variant, Variant::Demo);
        assert_eq!(
            select_variant(ProState::Dead, false).variant,
            Variant::Absent
        );
    }

    /// D021 的完整矩阵：三条「退回 v1」的路径都必须自带理由，
    /// 只有真正用上 v2 或彻底没通道时才允许 note 为空。
    #[test]
    fn every_demo_fallback_carries_its_reason() {
        let cases = [
            (ProState::NeedsToken, "需要 root"),
            (
                ProState::HandshakeFailed("v2 握手未通过".into()),
                "v2 握手未通过",
            ),
            (ProState::Dead, "未检测到 v2 模块"),
        ];
        for (pro, needle) in cases {
            let selection = select_variant(pro, true);
            assert_eq!(selection.variant, Variant::Demo, "{needle} 时应退回 demo");
            let note = selection
                .note
                .clone()
                .unwrap_or_else(|| panic!("退回 v1 必须带理由：{needle}"));
            assert!(note.contains(needle), "理由要能指向下一步：{note}");
            assert!(
                note.contains("指定 locale") && note.contains("不可用"),
                "必须同时说明 v1 缺什么能力：{note}"
            );
            assert!(!note.contains("  "), "理由文案不能有整段空格残留：{note:?}");
            // 退回后不得再声称能按请求 locale 解析
            assert!(selection.pro_locale.is_none());
        }
        // v2 可用：不带 note（没有降级要解释）
        let ready = select_variant(
            ProState::Ready(ProHello {
                version: "v2.0".into(),
                version_code: 2,
                locale: "zh-Hans-CN".into(),
                capabilities: vec!["list".into()],
                handlers: Vec::new(),
            }),
            true,
        );
        assert_eq!(ready.variant, Variant::Pro);
        assert!(ready.note.is_none() && ready.pro_locale.as_deref() == Some("zh-Hans-CN"));
        // v2 不可用且 v1 也不活：不是「退回」，不得伪装成 Demo 通道
        for pro in [ProState::NeedsToken, ProState::Dead] {
            let selection = select_variant(pro, false);
            assert_ne!(selection.variant, Variant::Demo);
        }
    }

    #[test]
    fn variant_decides_sub_protocol_and_module_id() {
        assert_eq!(Variant::Pro.sub_protocol(), PRO_SUB_PROTOCOL_VERSION);
        assert_eq!(Variant::Pro.module_id(), PRO_MODULE_ID);
        assert_eq!(Variant::Demo.sub_protocol(), DEMO_SUB_PROTOCOL_VERSION);
        assert_eq!(Variant::Demo.module_id(), DEMO_MODULE_ID);
        assert_eq!(Variant::Absent.sub_protocol(), 0);
    }

    /// AR10.1 兼容矩阵的四条规则都要能测：版本号严格、能力位是唯一增量通道、
    /// 陌生能力名不影响旧端、缺能力必须在发命令之前失败。
    #[test]
    fn v2_capability_gate_follows_the_compat_matrix() {
        // 规则 3：新模块多宣告了我们不认识的能力，旧 Agent 必须原样忽略而不是失败
        let new_module =
            parse_pro_hello("OK 2 2.1.0 21 zh-CN list manifest export device_info quantum")
                .expect("多出来的能力名不得让握手失败");
        assert_eq!(new_module.version, "2.1.0");
        assert!(
            new_module.capabilities.contains(&"quantum".to_owned()),
            "能力位原样保留，将来要用再加"
        );
        assert!(ensure_capability(&new_module, CAP_EXPORT, PACKAGE_EXPORT_APK).is_ok());

        // 规则 2：老模块没宣告 export → 拒，且要说清缺什么、它宣告了什么、模块版本
        let old = parse_pro_hello("OK 2 1.0.0 10 zh-CN list manifest").unwrap();
        let error = ensure_capability(&old, CAP_EXPORT, PACKAGE_EXPORT_APK)
            .expect_err("缺能力必须拒,不能把命令丢给模块再猜");
        assert_eq!(error.code, ErrorCode::UnsupportedMethod);
        let details = error.details.expect("必须带结构化理由");
        assert_eq!(details["reason"], "capability_missing");
        assert_eq!(details["capability"], "export");
        assert_eq!(details["module_version"], "1.0.0");
        assert_eq!(details["advertised"].as_array().expect("数组").len(), 2);
        // 一个能力都没宣告的模块（只握手不宣告）同样不能放行
        let bare = parse_pro_hello("OK 2 0.9 9 zh-CN").unwrap();
        assert!(ensure_capability(&bare, CAP_LIST, PACKAGE_LIST_LOCALIZED).is_err());
        // 状态查询这类不需要能力的命令走空串,别把门控做成一刀切
        assert!(ensure_capability(&bare, "", "zygisk.status").is_ok());

        // 规则 1：版本号严格匹配,低了高了都是 incompatible,不"试试看"
        for line in ["OK 1 1.0 1 zh-CN list", "OK 3 3.0 1 zh-CN list"] {
            let error = parse_pro_hello(line).expect_err("版本不匹配必须拒");
            assert_eq!(error.code, ErrorCode::IncompatibleVersion, "{line}");
        }
    }

    /// 模块自描述注册表的解析（AR10.2）：结构完整的帧转成 typed，坏帧与多余字段
    /// 都不得让整份 status 变红——模块侧以后长出新字段是常态，不是异常。
    #[test]
    fn handler_registry_frames_tolerate_module_side_growth() {
        let frames = vec![
            serde_json::json!({
                "cmd": "L", "capability": "list", "target": "system_server Java",
                "permission": "token", "timeout_ms": 8000, "max_response_bytes": 6291456,
                "cancellable": true, "fused": false
            }),
            serde_json::json!({
                "cmd": "E", "capability": "export", "target": "x", "permission": "token",
                "timeout_ms": 8000, "max_response_bytes": 1, "cancellable": false,
                "fused": true, "shiny_new_field": 42
            }),
            serde_json::json!({"capability": "list"}),
            serde_json::json!({"final": true}),
        ];
        let parsed = parse_handler_frames(&frames);
        assert_eq!(parsed.len(), 2, "只应收下结构完整的两条: {parsed:?}");
        assert_eq!(parsed[0].cmd, "L");
        assert_eq!(parsed[0].capability.as_deref(), Some("list"));
        assert!(!parsed[0].fused);
        assert_eq!(parsed[1].cmd, "E");
        assert!(
            parsed[1].fused,
            "熔断状态必须原样带回，界面上才说得出哪个方法被关了"
        );
    }

    #[test]
    fn pro_hello_requires_protocol_two_and_reads_caps() {
        let hello = parse_pro_hello("OK 2 v2.0 2 zh-Hans-CN list manifest export").unwrap();
        assert_eq!(hello.version, "v2.0");
        assert_eq!(hello.version_code, 2);
        assert_eq!(hello.locale, "zh-Hans-CN");
        assert_eq!(hello.capabilities, vec!["list", "manifest", "export"]);
        assert_eq!(
            parse_pro_hello("ERR auth_failed").unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
        assert_eq!(
            parse_pro_hello("OK 1 v1 1 zh-CN list").unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
    }

    #[test]
    fn pro_error_splits_on_whitespace_and_maps_codes() {
        // 模块在「无消息」时会留一个尾随空格，整串比较会误判
        let (code, message) = parse_pro_error("ERR auth_failed ").unwrap();
        assert_eq!(code, "auth_failed");
        assert_eq!(message, "");
        assert_eq!(
            pro_error_to_agent(&code, &message).code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            pro_error_to_agent("no_files", "com.x").code,
            ErrorCode::NotFound
        );
        assert_eq!(
            pro_error_to_agent("bad_locale", "$(id)").code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            pro_error_to_agent("unsupported_protocol", "").code,
            ErrorCode::IncompatibleVersion
        );
        assert_eq!(
            pro_error_to_agent("helper_failed", "boom").code,
            ErrorCode::Internal
        );
        assert!(parse_pro_error("F 1 com.x base.apk").is_none());
    }

    #[test]
    fn map_pro_items_keeps_module_evidence_and_counts_unproven() {
        let frames: Vec<serde_json::Value> = [
            // \u5929\u6c14 = 天气（真机 v2 实际报文形态）
            br#"{"pkg":"com.google.android.apps.weather","label":"\u5929\u6c14","labelSource":"framework","requestedLocale":"zh-Hans-CN","resolvedLocale":"zh-Hans-CN","fallbackReason":null,"versionName":"1.0","versionCode":34,"uid":10120,"isSystem":true,"enabled":true}"#.as_slice(),
            br#"{"pkg":"com.google.android.youtube","label":"YouTube","labelSource":"framework","requestedLocale":"fr-FR","resolvedLocale":null,"fallbackReason":"locale_not_resolved_fallback_default","versionName":"21.33","versionCode":1561,"uid":10233,"isSystem":false,"enabled":true}"#.as_slice(),
            br#"{"pkg":"com.google.android.overlay","label":"com.google.android.overlay","labelSource":"package_name","requestedLocale":"fr-FR","resolvedLocale":null,"fallbackReason":"label_equals_package_name","versionName":"1.0","versionCode":1,"uid":10073,"isSystem":true,"enabled":false}"#.as_slice(),
            br#"{"final":true,"count":3,"fallback":1,"localeUnproven":2,"deviceLocale":"zh-Hans-CN"}"#.as_slice(),
        ]
        .iter()
        .map(|raw| serde_json::from_slice(raw).unwrap())
        .collect();

        let (items, unproven, warnings) = map_pro_items(&frames, "fr-FR").unwrap();
        assert_eq!(items.len(), 3, "final 帧不应计入条目");
        assert!(warnings.is_empty());
        assert_eq!(items[0].label_source, LabelSource::Framework);
        assert_eq!(items[0].label, "天气");
        assert_eq!(items[0].resolved_locale.as_deref(), Some("zh-Hans-CN"));
        assert_eq!(items[0].uid, Some(10120));
        assert!(items[0].is_system && items[0].enabled);
        assert_eq!(items[1].version_name.as_deref(), Some("21.33"));
        assert_eq!(items[1].version_code, Some(1561));
        assert!(!items[1].is_system, "YouTube 应判为用户应用");
        assert_eq!(
            items[1].fallback_reason.as_deref(),
            Some("locale_not_resolved_fallback_default")
        );
        assert_eq!(items[2].label_source, LabelSource::PackageName);
        assert!(!items[2].enabled);
        assert_eq!(unproven, 2, "locale 无证据 + 包名回退各一条");
        assert_eq!(items[2].requested_locale, "fr-FR");
    }
    #[test]
    fn locale_compatibility_ignores_script_subtag() {
        assert!(locales_compatible("zh-CN", "zh-Hans-CN"));
        assert!(locales_compatible("zh", "zh-Hans-CN"));
        assert!(locales_compatible("zh-Hant-TW", "zh-Hant-TW"));
        assert!(!locales_compatible("zh-TW", "zh-Hans-CN"));
        assert!(!locales_compatible("en-US", "zh-Hans-CN"));
        assert!(!locales_compatible("", "zh-Hans-CN"));
    }

    #[test]
    fn apk_paths_classify_system_vs_user() {
        assert_eq!(
            classify_system(&["/data/app/~~abc==/com.x-def/base.apk"]),
            Some(false)
        );
        assert_eq!(classify_system(&["/product/overlay/x.apk"]), Some(true));
        assert_eq!(classify_system(&["apex/x.apk"]), None);
        assert_eq!(classify_system(&[]), None);
    }

    #[test]
    fn pm_lines_keep_uids_and_disabled_state() {
        let mut flags = PmFlags::default();
        collect_pm_lines(
            "package:com.a uid:10152\r\npackage:com.b\r\n\r\n",
            &mut flags,
            false,
        );
        collect_pm_lines("package:com.c uid:10073\n", &mut flags, true);
        assert_eq!(flags.uids.get("com.a").copied(), Some(10152));
        assert!(flags.enabled.contains("com.b"));
        assert_eq!(flags.uids.get("com.b"), None);
        assert!(flags.disabled.contains("com.c"));
        assert!(!flags.enabled.contains("com.c"));
    }

    #[test]
    fn module_json_shapes_are_typed_or_incompatible() {
        let apps = parse_apps(
            br#"[{"pkg":"com.x","label":"\u6d4b\u8bd5","versionName":"1.2","versionCode":12}]"#
                .as_slice(),
        )
        .unwrap();
        assert_eq!(apps[0].pkg, "com.x");
        assert_eq!(apps[0].label, "测试");
        assert_eq!(apps[0].version_code, Some(12));

        let error = parse_apps(br#"{"error":"helper crashed"}"#).unwrap_err();
        assert!(error.message.contains("helper crashed"));
        assert_eq!(
            parse_apps(br#"{"unexpected":1}"#).unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
        assert!(parse_manifest(br#"{"error":"x"}"#).unwrap().is_empty());
    }

    #[test]
    fn one_bad_item_does_not_lose_the_whole_list() {
        let raw_frames: Vec<&[u8]> = vec![
            br#"{"pkg":"com.good","label":"\u597d\u7684","labelSource":"framework","resolvedLocale":"zh-Hans-CN","versionName":"1.0","versionCode":1,"uid":10001,"isSystem":false,"enabled":true}"#.as_slice(),
            // versionCode 类型不对（真机 helper 异常时可能是字符串或缺失）
            br#"{"pkg":"com.broken","label":"\u574f\u7684","versionCode":"NaN","isSystem":false}"#.as_slice(),
            br#"{"label":"\u6ca1\u6709\u5305\u540d","versionCode":2}"#.as_slice(),
            br#"{"final":true,"count":3,"fallback":1,"deviceLocale":"zh-Hans-CN"}"#.as_slice(),
        ];
        let frames: Vec<serde_json::Value> = raw_frames
            .iter()
            .map(|raw| serde_json::from_slice(raw).unwrap())
            .collect();

        let (items, unproven, warnings) = map_pro_items(&frames, "zh-Hans-CN").unwrap();
        assert_eq!(items.len(), 1, "坏条目跳过但整批必须继续");
        assert_eq!(items[0].package_name, "com.good");
        assert_eq!(items[0].label, "好的");
        assert_eq!(
            warnings.len(),
            2,
            "两条坏数据各出一条 item warning: {warnings:?}"
        );
        assert_eq!(warnings[0].package_name.as_deref(), Some("com.broken"));
        assert_eq!(warnings[0].code, "item_parse_failed");
        assert_eq!(warnings[1].package_name, None, "缺包名时不得编造");
        assert_eq!(unproven, 0);
    }

    #[test]
    fn export_headers_validate_every_field() {
        let (size, pkg, name) =
            parse_export_header(b"F 29085 com.example NoCutoutOverlay.apk").unwrap();
        assert_eq!(
            (size, pkg.as_str(), name.as_str()),
            (29085, "com.example", "NoCutoutOverlay.apk")
        );
        assert_eq!(
            parse_export_header(b"F n/a com.example x.apk")
                .unwrap_err()
                .code,
            ErrorCode::IncompatibleVersion
        );
        assert!(validate_package_name("com.example;rm -rf /").is_err());
        assert!(validate_file_name("../escape.apk").is_err());
        assert!(validate_file_name("split_config.zh.apk").is_ok());
        assert!(validate_session("0123456789abcdef").is_ok());
        assert!(validate_session("../../tmp").is_err());
        assert!(validate_session("0123456789abcde").is_err());
    }

    #[tokio::test]
    async fn capabilities_report_zygisk_availability_from_probe_cache() {
        let provider = Arc::new(ZygiskProvider::new());
        // 未探测：仅诊断方法可用，业务方法必须显式不可用，避免假成功。
        assert_eq!(provider.unavailable_reason(ZYGISK_STATUS), None);
        assert!(
            provider
                .unavailable_reason(PACKAGE_LIST_LOCALIZED)
                .is_some()
        );
        assert_eq!(provider.info().health, ProviderHealth::Unavailable);
        // 未探测 = 无通道：版本串显式为「未知模块 - 子协议 0」，不给假的 v1 认定
        assert_eq!(provider.info().version, "-sub-0".to_owned());
        assert_eq!(
            provider.info().last_error.as_deref(),
            Some("zygisk bridge 尚未探测")
        );
    }
}
