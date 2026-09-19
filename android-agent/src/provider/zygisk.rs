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
    AgentError, ErrorCode, LabelSource, LocalizedPackageItem, PackageExportApkParams,
    PackageExportApkResult, PackageExportCleanParams, PackageExportCleanResult,
    PackageListLocalizedParams, PackageListLocalizedResult, PackageScope, PackageWarning,
    ProviderHealth, ProviderInfo, StagedApkFile, ZygiskLifecycle, ZygiskStatusResult,
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

/// 模块标识：决定 `/data/adb/modules/<MODULE_ID>` 与 bridge 语义。
pub const MODULE_ID: &str = "applist";
/// Agent <-> 模块私有子协议版本（Q/E/D 线协议冻结为 1）。
pub const SUB_PROTOCOL_VERSION: u32 = 1;

const MODULE_HOST: &str = "127.0.0.1";
const MODULE_PORT: u16 = 11_500;
/// 模块内部响应缓冲 4 MiB，留出余量后作为单行上限。
const MAX_LINE_BYTES: u64 = 6 * 1024 * 1024;
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const STAGING_ROOT: &str = "/data/local/tmp/app-reverse-tools-apk";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const LINE_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TTL: Duration = Duration::from_secs(5);
const ROOT_TIMEOUT: Duration = Duration::from_millis(2500);

pub struct ZygiskProvider {
    probe: Mutex<Option<Probe>>,
}

#[derive(Debug, Clone)]
struct RootFacts {
    module_installed: bool,
    pending_update: bool,
    disabled_marker: bool,
    remove_marker: bool,
    zygisk_impl: Option<String>,
    module_version: Option<String>,
    module_version_code: Option<u32>,
}

#[derive(Debug, Clone)]
struct Probe {
    at: Instant,
    lifecycle: ZygiskLifecycle,
    bridge_ready: bool,
    root: Option<RootFacts>,
    device_locale: Option<String>,
    probe_latency_ms: u64,
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
        }
    }

    /// Agent 启动后先探一次，保证首个 `system.hello` 就带真实可用性。
    pub async fn warm_up(&self) {
        self.probe_now().await;
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
        let bridge_ready = bridge_alive(CONNECT_TIMEOUT).await;
        let root = probe_root().await;
        let device_locale = detect_device_locale().await;
        let lifecycle = classify_lifecycle(root.as_ref(), bridge_ready);
        let probe = Probe {
            at: Instant::now(),
            lifecycle: lifecycle.0,
            bridge_ready,
            root,
            device_locale,
            probe_latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            detail: lifecycle.1,
        };
        self.store(probe.clone());
        probe
    }

    async fn probe(&self) -> Probe {
        if let Some(cached) = self.cached() {
            if cached.at.elapsed() < PROBE_TTL {
                return cached;
            }
        }
        self.probe_now().await
    }

    fn status(&self, probe: &Probe) -> ZygiskStatusResult {
        ZygiskStatusResult {
            lifecycle: probe.lifecycle,
            bridge_ready: probe.bridge_ready,
            root_available: probe.root.is_some(),
            module_id: Some(MODULE_ID.to_owned()),
            module_version: probe
                .root
                .as_ref()
                .and_then(|facts| facts.module_version.clone()),
            module_version_code: probe
                .root
                .as_ref()
                .and_then(|facts| facts.module_version_code),
            zygisk_impl: probe
                .root
                .as_ref()
                .and_then(|facts| facts.zygisk_impl.clone()),
            device_locale: probe.device_locale.clone(),
            sub_protocol_version: SUB_PROTOCOL_VERSION,
            probe_latency_ms: Some(probe.probe_latency_ms),
            detail: probe.detail.clone(),
        }
    }

    async fn handle_status(&self, params: Value) -> Result<Value, AgentError> {
        let _probe_params: agent_protocol::ZygiskStatusParams = parse_params(params)?;
        let probe = self.probe_now().await;
        serialize(&self.status(&probe))
    }

    async fn handle_list(&self, params: Value) -> Result<Value, AgentError> {
        let params: PackageListLocalizedParams = parse_params(params)?;
        let probe = self.probe().await;
        require_bridge(&probe)?;

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

        let mut stream = connect_bridge_stream(CONNECT_TIMEOUT).await?;
        let staged = stage_package_export(&mut stream, &params.package_name, &staging_dir).await;
        drop(stream);
        let (files, bytes) = match staged {
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
            version: format!("sub-{SUB_PROTOCOL_VERSION}"),
            health,
            required_permissions: vec!["zygisk_module".into()],
            last_error,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        ZYGISK_METHODS
    }

    fn unavailable_reason(&self, method: &str) -> Option<String> {
        // 生命周期诊断本身永远可用，否则 UI 拿不到「为什么不可用」。
        if method == ZYGISK_STATUS {
            return None;
        }
        match self.cached() {
            None => Some("zygisk bridge 尚未探测完成".to_owned()),
            Some(probe) if probe.bridge_ready => None,
            Some(probe) => Some(
                probe
                    .detail
                    .clone()
                    .unwrap_or_else(|| format!("Zygisk 模块状态为 {:?}", probe.lifecycle)),
            ),
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

async fn bridge_alive(timeout: Duration) -> bool {
    matches!(connect_bridge_stream(timeout).await, Ok(_stream))
}

async fn connect_bridge_stream(timeout: Duration) -> Result<TcpStream, AgentError> {
    let address = format!("{MODULE_HOST}:{MODULE_PORT}");
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
/// 返回（文件列表, 总字节数）；失败清理由调用方负责。
async fn stage_package_export(
    stream: &mut TcpStream,
    package_name: &str,
    staging_dir: &str,
) -> Result<(Vec<StagedApkFile>, u64), AgentError> {
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
            return Ok((files, total));
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
const ROOT_PROBE_SCRIPT: &str = concat!(
    "echo ROOT=1;",
    "if [ -d /data/adb/modules/applist ]; then echo MODULE=1; fi;",
    "if [ -d /data/adb/modules_update/applist ]; then echo PENDING_UPDATE=1; fi;",
    "if [ -f /data/adb/modules/applist/disable ]; then echo DISABLED=1; fi;",
    "if [ -f /data/adb/modules/applist/remove ]; then echo REMOVING=1; fi;",
    "for d in /data/adb/zygisksu /data/adb/zygisk /data/adb/ap/zygisk /data/adb/kzygisk; do",
    " if [ -e \"$d\" ]; then echo \"ZYGIMPL=$(basename \"$d\")\"; fi; done;",
    "if [ -f /data/adb/modules/applist/module.prop ]; then echo PROP_BEGIN;",
    " cat /data/adb/modules/applist/module.prop; echo PROP_END; fi;",
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
    let mut facts = RootFacts {
        module_installed: false,
        pending_update: false,
        disabled_marker: false,
        remove_marker: false,
        zygisk_impl: None,
        module_version: None,
        module_version_code: None,
    };
    let mut in_prop = false;
    for line in text.lines().map(str::trim) {
        if in_prop {
            if line == "PROP_END" {
                in_prop = false;
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "version" => facts.module_version = Some(value.to_owned()),
                    "versionCode" => facts.module_version_code = value.parse().ok(),
                    _ => {}
                }
            }
            continue;
        }
        match line {
            "MODULE=1" => facts.module_installed = true,
            "PENDING_UPDATE=1" => facts.pending_update = true,
            "DISABLED=1" => facts.disabled_marker = true,
            "REMOVING=1" => facts.remove_marker = true,
            "PROP_BEGIN" => in_prop = true,
            other => {
                if let Some(value) = other.strip_prefix("ZYGIMPL=") {
                    if facts.zygisk_impl.is_none() && !value.is_empty() {
                        facts.zygisk_impl = Some(value.to_owned());
                    }
                }
            }
        }
    }
    facts
}

fn classify_lifecycle(
    root: Option<&RootFacts>,
    bridge_ready: bool,
) -> (ZygiskLifecycle, Option<String>) {
    let Some(facts) = root else {
        return if bridge_ready {
            (
                ZygiskLifecycle::BridgeReady,
                Some("root 不可用，模块安装状态未探测；bridge 已响应".to_owned()),
            )
        } else {
            (
                ZygiskLifecycle::Faulted,
                Some("root 不可用且 bridge 未响应，无法区分未安装/未启用/需重启".to_owned()),
            )
        };
    };
    if bridge_ready {
        if facts.pending_update {
            return (
                ZygiskLifecycle::InstalledRebootRequired,
                Some("模块有新版本待重启加载（bridge 仍是旧 .so）".to_owned()),
            );
        }
        return (ZygiskLifecycle::BridgeReady, None);
    }
    if facts.remove_marker {
        return (
            ZygiskLifecycle::NotInstalled,
            Some("模块已标记删除，将在下次启动移除".to_owned()),
        );
    }
    if !facts.module_installed {
        return (
            ZygiskLifecycle::NotInstalled,
            Some(format!(
                "/data/adb/modules/{MODULE_ID} 不存在，需安装模块 ZIP"
            )),
        );
    }
    if facts.disabled_marker || facts.zygisk_impl.is_none() {
        return (
            ZygiskLifecycle::ZygiskDisabled,
            Some(
                "模块或 Zygisk 实现被禁用（检查 KernelSU/Magisk 的 Zygisk 开关与模块 enable）"
                    .to_owned(),
            ),
        );
    }
    if facts.pending_update {
        return (
            ZygiskLifecycle::InstalledRebootRequired,
            Some("模块已安装/升级，需重启加载新的 zygisk .so".to_owned()),
        );
    }
    (
        ZygiskLifecycle::Loaded,
        Some("模块已加载但 bridge 未监听（system_server 侧握手未完成）".to_owned()),
    )
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
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("package:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let Some(package) = parts.next() else {
            continue;
        };
        let package = package.to_owned();
        if disabled {
            flags.disabled.insert(package.clone());
        } else {
            flags.enabled.insert(package.clone());
        }
        if let Some(uid) = parts
            .find_map(|token| token.strip_prefix("uid:"))
            .and_then(|value| value.parse::<u32>().ok())
        {
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

    const PROP_SAMPLE: &str = concat!(
        "ROOT=1\n",
        "MODULE=1\n",
        "ZYGIMPL=zygisksu\n",
        "PROP_BEGIN\n",
        "id=applist\n",
        "name=Applist Zygisk\n",
        "version=v1.0\n",
        "versionCode=1\n",
        "PROP_END\n",
        "PROBE_DONE=1\n",
    );

    #[test]
    fn root_probe_parses_markers_and_module_prop() {
        let facts = classify_root_output(PROP_SAMPLE);
        assert!(facts.module_installed);
        assert!(!facts.pending_update);
        assert_eq!(facts.zygisk_impl.as_deref(), Some("zygisksu"));
        assert_eq!(facts.module_version.as_deref(), Some("v1.0"));
        assert_eq!(facts.module_version_code, Some(1));
    }

    #[test]
    fn pending_update_outweighs_bridge_readiness() {
        let facts = classify_root_output(
            &PROP_SAMPLE
                .to_string()
                .replace("MODULE=1", "MODULE=1\nPENDING_UPDATE=1"),
        );
        assert!(facts.pending_update);
        let (lifecycle, detail) = classify_lifecycle(Some(&facts), true);
        assert_eq!(lifecycle, ZygiskLifecycle::InstalledRebootRequired);
        assert!(detail.unwrap().contains("旧 .so"));

        let (loaded, _) = classify_lifecycle(Some(&facts), false);
        assert_eq!(loaded, ZygiskLifecycle::InstalledRebootRequired);
    }

    #[test]
    fn lifecycle_without_root_stays_honest() {
        let (ready, detail) = classify_lifecycle(None, true);
        assert_eq!(ready, ZygiskLifecycle::BridgeReady);
        assert!(detail.unwrap().contains("root 不可用"));
        let (unknown, detail) = classify_lifecycle(None, false);
        assert_eq!(unknown, ZygiskLifecycle::Faulted);
        assert!(detail.unwrap().contains("无法区分"));
    }

    #[test]
    fn lifecycle_distinguishes_missing_module_from_disabled_zygisk() {
        let mut facts = classify_root_output(PROP_SAMPLE);
        facts.module_installed = false;
        assert_eq!(
            classify_lifecycle(Some(&facts), false).0,
            ZygiskLifecycle::NotInstalled
        );
        facts.module_installed = true;
        facts.zygisk_impl = None;
        assert_eq!(
            classify_lifecycle(Some(&facts), false).0,
            ZygiskLifecycle::ZygiskDisabled
        );
        facts.zygisk_impl = Some("zygisksu".into());
        assert_eq!(
            classify_lifecycle(Some(&facts), false).0,
            ZygiskLifecycle::Loaded
        );
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
            br#"[{"pkg":"com.x","label":"\u6d4b\u8bd5","versionName":"1.2","versionCode":12}]"#,
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
        assert_eq!(
            provider.info().version,
            format!("sub-{SUB_PROTOCOL_VERSION}")
        );
    }
}
