//! Zygisk 应用清单 Service（AR5.3/AR5.4）。
//!
//! 边界：模块的 `Q/E/D` 私有线协议由 **Android Agent 的 ZygiskProvider** 负责，
//! Desktop 只调用 Agent typed API，不再直连模块端口；APK 字节回传复用
//! 「Desktop Transport」允许项（ADB pull），因此本文件不出现任何 Zygisk 私有协议。
//! 该能力不允许静默降级成 `pm`/Shell 结果（AR5.5 规则）：缺 Agent 或缺模块时
//! 直接返回可诊断的 `provider_unavailable`。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_protocol::method::{
    PACKAGE_DESCRIBE, PACKAGE_EXPORT_APK, PACKAGE_EXPORT_CLEAN, PACKAGE_LIST_LOCALIZED,
    ZYGISK_STATUS,
};
use agent_protocol::{
    PackageDescribeParams, PackageDescribeResult, PackageExportApkParams, PackageExportApkResult,
    PackageExportCleanParams, PackageListLocalizedParams, PackageListLocalizedResult, PackageScope,
    ZygiskStatusResult,
};
use serde::{Deserialize, Serialize};

use crate::adapters::adb;
use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{AgentSessionState, AndroidBackendSource};
use crate::services::android_backend::{AgentBackendError, CapabilityRouter, OperationKind};
use crate::services::apk_bundle::{self, AppNaming, PulledPart};
use crate::services::device_service::AdbRunner;

const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(900);
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(20);
const PULL_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalizedScope {
    All,
    User,
    System,
}

impl LocalizedScope {
    fn as_protocol(self) -> PackageScope {
        match self {
            Self::All => PackageScope::All,
            Self::User => PackageScope::User,
            Self::System => PackageScope::System,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskAppItem {
    pub package_name: String,
    pub label: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    pub label_source: String,
    pub requested_locale: String,
    pub resolved_locale: Option<String>,
    pub fallback_reason: Option<String>,
    pub uid: Option<u32>,
    pub is_system: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskAppWarning {
    pub package_name: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskAppList {
    pub items: Vec<ZygiskAppItem>,
    pub success_count: u32,
    pub fallback_count: u32,
    pub warnings: Vec<ZygiskAppWarning>,
    pub device_locale: Option<String>,
    /// `zygisk_v2` / `zygisk_v1` / `zygisk_none`：UI 必须能看出当前是否走了降级通道
    pub channel: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskStatus {
    /// `not_installed` / `zygisk_disabled` / `installed_reboot_required` /
    /// `loaded` / `bridge_ready` / `incompatible` / `faulted`
    pub lifecycle: String,
    pub bridge_ready: bool,
    pub root_available: bool,
    pub agent_connected: bool,
    pub module_id: Option<String>,
    pub module_version: Option<String>,
    pub module_version_code: Option<u32>,
    pub zygisk_impl: Option<String>,
    pub device_locale: Option<String>,
    pub sub_protocol_version: u32,
    pub probe_latency_ms: Option<u64>,
    /// 模块自描述的 handler 注册表（AR10.2）。这里过一层 camelCase 映射，
    /// 是因为界面对象一直是本文件的模型，不该看见设备私有协议的字段命名。
    pub module_handlers: Vec<ModuleHandlerDto>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleHandlerDto {
    pub cmd: String,
    pub capability: Option<String>,
    pub target: String,
    pub permission: String,
    pub timeout_ms: u64,
    pub max_response_bytes: u64,
    pub cancellable: bool,
    pub fused: bool,
}

impl From<agent_protocol::ModuleHandlerInfo> for ModuleHandlerDto {
    fn from(v: agent_protocol::ModuleHandlerInfo) -> Self {
        Self {
            cmd: v.cmd,
            capability: v.capability,
            target: v.target,
            permission: v.permission,
            timeout_ms: v.timeout_ms,
            max_response_bytes: v.max_response_bytes,
            cancellable: v.cancellable,
            fused: v.fused,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskApkFile {
    pub package_name: String,
    pub name: String,
    pub size: u64,
}

/// 导出结果：一次导出的**产物**（单个 `.apk` 或 `.apks`）+ 它的组成明细。
///
/// 命名规则由 `apk_bundle` 决定：无分包 -> `<显示名>_<版本号>.apk`，
/// 有分包 -> 合并成 SAI 格式的 `<显示名>_<版本号>.apks`。显示名与版本号是向 Zygisk
/// 现查的（设备语言为中文即中文名），不采用界面回传的字符串，避免用旧值命名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskExportReport {
    pub package_name: String,
    /// `apk` = 无分包单文件；`apks` = 分包已合并进 SAI 的 .apks 容器
    pub kind: String,
    pub file_name: String,
    pub artifact_path: String,
    pub artifact_bytes: u64,
    /// 产物里的各分片（保持设备侧原名，便于回溯是哪个 split）
    pub parts: Vec<ZygiskApkFile>,
    /// 各分片原始字节之和（不含 zip 头与两份 SAI meta）
    pub bytes: u64,
    pub app_label: String,
    pub version_name: String,
    /// `zygisk_framework` / `zygisk_manifest` / `zygisk_package_name` /
    /// `fallback_not_in_list` / `fallback_list_unavailable`
    pub name_source: String,
    /// 产物里缺的分片：模块明确跳过的（`too_large`）+ 设备侧声明了但没取回的。
    /// 非空表示产物不完整，界面必须说清楚——合成单文件后少一个分包肉眼看不出来。
    pub skipped: Vec<String>,
    pub complete: bool,
    /// 设备侧声明的分片总数（问不到时为 null，不参与完整性判断）
    pub declared_parts: Option<u64>,
    pub destination: String,
}

/// 一次导出前问到的权威元数据：命名用的名字/版本 + 设备侧声明的分包清单。
#[derive(Debug, Clone)]
struct PackageAuthority {
    naming: AppNaming,
    /// `None` = 这条路径拿不到分包集合（老模块退回批量清单时），不参与缺件判断
    declared: Option<Vec<agent_protocol::DescribedApkFile>>,
}

pub struct ZygiskApplistService {
    android: Arc<CapabilityRouter>,
    runner: Arc<dyn AdbRunner>,
}

impl ZygiskApplistService {
    pub fn new(android: Arc<CapabilityRouter>, runner: Arc<dyn AdbRunner>) -> Self {
        Self { android, runner }
    }

    /// 诊断模块生命周期状态；UI 用它区分「未安装 / 未启用 / 需重启 / bridge 未就绪」。
    pub async fn status(&self, serial: &str) -> CoreResult<ZygiskStatus> {
        let state = self.android.agent_status(serial).state;
        let agent_connected = matches!(
            state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        );
        self.require_agent(serial, ZYGISK_STATUS)?;
        let result: ZygiskStatusResult = self
            .android
            .agent()
            .request(
                serial,
                ZYGISK_STATUS,
                &serde_json::json!({}),
                STATUS_TIMEOUT,
            )
            .await
            .map_err(agent_core_error)?;
        Ok(ZygiskStatus {
            lifecycle: lifecycle_name(result.lifecycle).to_owned(),
            bridge_ready: result.bridge_ready,
            root_available: result.root_available,
            agent_connected,
            module_id: result.module_id,
            module_version: result.module_version,
            module_version_code: result.module_version_code,
            zygisk_impl: result.zygisk_impl,
            device_locale: result.device_locale,
            sub_protocol_version: result.sub_protocol_version,
            probe_latency_ms: result.probe_latency_ms,
            module_handlers: result
                .module_handlers
                .into_iter()
                .map(ModuleHandlerDto::from)
                .collect(),
            detail: result.detail,
        })
    }

    /// `package.list_localized`：一次批量 RPC 取得 Framework 解析后的显示名。
    pub async fn list(
        &self,
        serial: &str,
        locale: Option<String>,
        scope: LocalizedScope,
        include_disabled: bool,
    ) -> CoreResult<ZygiskAppList> {
        self.require_agent(serial, PACKAGE_LIST_LOCALIZED)?;
        let params = PackageListLocalizedParams {
            locale,
            scope: scope.as_protocol(),
            include_disabled,
        };
        let result: PackageListLocalizedResult = self
            .android
            .agent()
            .request(serial, PACKAGE_LIST_LOCALIZED, &params, LIST_TIMEOUT)
            .await
            .map_err(agent_core_error)?;
        Ok(map_localized_result(result))
    }

    /// 导出一个应用的 APK：Agent 在设备侧暂存 -> Desktop 经 ADB pull 取回并校验大小
    /// -> 宿主侧装配成单个产物（无分包改名成 `.apk`，有分包合并成 `.apks`）。
    ///
    /// 装配失败时保留暂存目录里的原件（用户至少还能拿到散装 split），
    /// 但会把错误照实抛出，不伪装成“已保存”。
    pub async fn export_package(
        &self,
        serial: &str,
        package_name: &str,
        destination: &Path,
    ) -> CoreResult<ZygiskExportReport> {
        validate_package_name(package_name)?;
        self.require_agent(serial, PACKAGE_EXPORT_APK)?;

        let staged: PackageExportApkResult = self
            .android
            .agent()
            .request(
                serial,
                PACKAGE_EXPORT_APK,
                &PackageExportApkParams {
                    package_name: package_name.to_owned(),
                },
                EXPORT_TIMEOUT,
            )
            .await
            .map_err(agent_core_error)?;

        // 命名与分包校验用的权威元数据现查现用：这一步失败只降级成包名命名，
        // 不影响文件取回。
        let authority = self.authority_for(serial, package_name).await;
        let naming = authority.naming.clone();

        tokio::fs::create_dir_all(destination)
            .await
            .map_err(|error| CoreError::Internal(format!("创建导出目录失败: {error}")))?;
        let stage_dir = stage_dir_for(destination, package_name, &staged.session);
        tokio::fs::create_dir_all(&stage_dir)
            .await
            .map_err(|error| CoreError::Internal(format!("创建导出暂存目录失败: {error}")))?;

        let pull = self.pull_staged_files(serial, &staged, &stage_dir).await;
        // 无论取回成功与否都回收设备侧暂存，避免残留 APK 副本。
        if let Err(error) = self.clean_staged(serial, &staged.session).await {
            tracing::warn!(serial, error = %error, "Zygisk 导出暂存清理失败");
        }
        let pulled = pull?;

        let artifact = match apk_bundle::assemble(&naming, pulled.clone(), destination).await {
            Ok(artifact) => artifact,
            Err(error) => {
                // 半截产物由 assemble 自己清掉；散装原件留在暂存目录里，
                // 把路径写进错误信息，用户还能手工取用
                return Err(CoreError::Internal(format!(
                    "{error}（未合并的原始文件保留在 {}）",
                    stage_dir.display()
                )));
            }
        };
        if let Err(error) = tokio::fs::remove_dir_all(&stage_dir).await {
            tracing::warn!(error = %error, "导出装配暂存目录清理失败");
        }

        let parts: Vec<ZygiskApkFile> = pulled
            .iter()
            .map(|part| ZygiskApkFile {
                package_name: staged.package_name.clone(),
                name: part.name.clone(),
                size: part.size,
            })
            .collect();
        let bytes = pulled.iter().map(|part| part.size).sum::<u64>();

        // 缺件判定：以设备侧声明的那份分包清单为准（见 missing_parts 的注释）
        let skipped = missing_parts(authority.declared.as_deref(), &pulled, &staged.skipped);

        Ok(ZygiskExportReport {
            package_name: staged.package_name,
            kind: artifact.kind.as_str().to_owned(),
            file_name: artifact.file_name,
            artifact_path: artifact.path.to_string_lossy().into_owned(),
            artifact_bytes: artifact.bytes,
            parts,
            bytes,
            app_label: naming.label,
            version_name: naming.version_name,
            name_source: naming.name_source,
            declared_parts: authority.declared.as_ref().map(|files| files.len() as u64),
            complete: skipped.is_empty(),
            skipped,
            destination: destination.to_string_lossy().into_owned(),
        })
    }

    /// 产物命名与分包校验要用的权威元数据：优先单点问 `package.describe`，
    /// 老模块没这条能力时退回 `package.list_localized` 查同一个包。
    ///
    /// 两条路的显示名都必须来自 Framework（设备语言为中文即中文名）——ADB 与 `pm`
    /// 给不出中文名，所以**不允许**再退回界面回传的字符串或本地猜测。
    async fn authority_for(&self, serial: &str, package_name: &str) -> PackageAuthority {
        match self.describe(serial, package_name).await {
            Ok(described) => {
                tracing::debug!(serial, package_name, "APK 导出命名走按包查询");
                PackageAuthority {
                    declared: Some(described.apk_files),
                    naming: AppNaming {
                        package_name: described.package_name,
                        label: described.label,
                        version_name: described.version_name.unwrap_or_default(),
                        version_code: described.version_code,
                        name_source: "zygisk_describe".to_owned(),
                    },
                }
            }
            Err(error) => {
                // 能力缺失不是故障：v2.0 及更早的模块没有 P 命令，退回批量清单。
                // 包不存在也走同一条路（清单里同样找不到 -> 用包名命名），
                // 而真正会挡住用户的是后面那次导出本身的错误，它照样会抛出。
                tracing::warn!(
                    serial,
                    package_name,
                    error = %error,
                    "按包查询没拿到结果，退回批量本地化清单取命名（分包集合无法校验）"
                );
                let naming = self.naming_from_list(serial, package_name).await;
                PackageAuthority {
                    declared: None,
                    naming,
                }
            }
        }
    }

    /// `package.describe`：单点问 Framework 某个包的显示名、版本与分包集合。
    pub async fn describe(
        &self,
        serial: &str,
        package_name: &str,
    ) -> CoreResult<PackageDescribeResult> {
        validate_package_name(package_name)?;
        self.require_agent(serial, PACKAGE_DESCRIBE)?;
        self.android
            .agent()
            .request(
                serial,
                PACKAGE_DESCRIBE,
                &PackageDescribeParams {
                    package_name: package_name.to_owned(),
                },
                DESCRIBE_TIMEOUT,
            )
            .await
            .map_err(agent_core_error)
    }

    /// 兜底路径：从批量本地化清单里捞这一个包。代价是多一次全机清单。
    async fn naming_from_list(&self, serial: &str, package_name: &str) -> AppNaming {
        let fallback = |reason: &str| AppNaming {
            package_name: package_name.to_owned(),
            label: package_name.to_owned(),
            version_name: String::new(),
            version_code: None,
            name_source: reason.to_owned(),
        };
        match self
            .list(serial, None, LocalizedScope::All, true)
            .await
            .map(|list| {
                list.items
                    .into_iter()
                    .find(|item| item.package_name == package_name)
            }) {
            Ok(Some(item)) => AppNaming {
                package_name: package_name.to_owned(),
                label: item.label,
                version_name: item.version_name,
                version_code: item.version_code,
                name_source: format!("zygisk_list_{}", item.label_source),
            },
            Ok(None) => {
                tracing::warn!(
                    serial,
                    package_name,
                    "Zygisk 清单里没有该包，产物改用包名命名"
                );
                fallback("fallback_not_in_list")
            }
            Err(error) => {
                tracing::warn!(
                    serial,
                    package_name,
                    error = %error,
                    "查询 Zygisk 清单失败，产物改用包名命名"
                );
                fallback("fallback_list_unavailable")
            }
        }
    }

    async fn pull_staged_files(
        &self,
        serial: &str,
        staged: &PackageExportApkResult,
        package_dir: &Path,
    ) -> CoreResult<Vec<PulledPart>> {
        let environment = self.runner.environment().await;
        let adb_path = environment
            .path
            .ok_or_else(|| CoreError::Internal("adb 不可用，无法取回导出的 APK".into()))?;
        let mut files = Vec::with_capacity(staged.files.len());
        for entry in &staged.files {
            let name = safe_filename(&entry.name)?;
            let target = package_dir.join(&name);
            let output = self
                .runner
                .run(
                    &adb_path,
                    &adb::build_args(
                        Some(serial),
                        &adb::cmd_pull(&entry.remote_path, &target.to_string_lossy()),
                    ),
                    PULL_TIMEOUT,
                )
                .await?;
            if output.exit_code != Some(0) {
                return Err(CoreError::Internal(format!(
                    "取回 {} 失败: {}",
                    entry.remote_path,
                    output.stderr.trim()
                )));
            }
            let metadata = tokio::fs::metadata(&target)
                .await
                .map_err(|error| CoreError::Internal(format!("读取取回文件失败: {error}")))?;
            if metadata.len() != entry.size {
                return Err(CoreError::Internal(format!(
                    "{} 大小不符：设备侧 {}，本地 {}",
                    name,
                    entry.size,
                    metadata.len()
                )));
            }
            files.push(PulledPart {
                name,
                path: target,
                size: entry.size,
            });
        }
        Ok(files)
    }

    async fn clean_staged(&self, serial: &str, session: &str) -> CoreResult<()> {
        let _: agent_protocol::PackageExportCleanResult = self
            .android
            .agent()
            .request(
                serial,
                PACKAGE_EXPORT_CLEAN,
                &PackageExportCleanParams {
                    session: session.to_owned(),
                },
                STATUS_TIMEOUT,
            )
            .await
            .map_err(agent_core_error)?;
        Ok(())
    }

    /// 仅真机测试用：透传到公共 `package.list`，验证它与 Zygisk 无关也能工作。
    #[cfg(test)]
    pub async fn plain_packages_for_test(&self, serial: &str) -> CoreResult<Vec<String>> {
        use crate::services::android_backend::{AgentBackendError, CapabilityRouter};
        let result = self
            .android
            .agent()
            .request::<_, agent_protocol::PackageListResult>(
                serial,
                agent_protocol::method::PACKAGE_LIST,
                &agent_protocol::PackageListParams {
                    scope: agent_protocol::PackageScope::All,
                    include_disabled: false,
                },
                LIST_TIMEOUT,
            )
            .await
            .map_err(|error| match error {
                AgentBackendError::Business(business) => CoreError::Internal(business.message),
                other => CapabilityRouter::agent_error(other),
            })?;
        Ok(result
            .items
            .into_iter()
            .map(|item| item.package_name)
            .collect())
    }

    /// 三个方法都不登记 Legacy 回退：Agent 未连接或模块缺失时必须显式失败。
    fn require_agent(&self, serial: &str, method: &str) -> CoreResult<()> {
        match self
            .android
            .select(serial, method, OperationKind::ReadOnlyIdempotent)
        {
            Ok(decision) if decision.backend == AndroidBackendSource::Agent => Ok(()),
            Ok(decision) => Err(CoreError::AgentUnavailable(format!(
                "Zygisk 能力不能由 Legacy ADB 提供，但路由选择了 {:?}",
                decision.backend
            ))),
            Err(error) => Err(route_error(method, error)),
        }
    }
}

/// Agent 业务错误里的 `provider_unavailable` / `incompatible_version` 是「能力缺失」
/// 而不是内部故障，必须映射成可路由的 CoreError 变体，UI 才能给安装/启用/重启指引。
fn agent_core_error(error: AgentBackendError) -> CoreError {
    match &error {
        AgentBackendError::Business(business)
            if business.code == agent_protocol::ErrorCode::ProviderUnavailable =>
        {
            CoreError::AgentUnavailable(business.message.clone())
        }
        AgentBackendError::Business(business)
            if business.code == agent_protocol::ErrorCode::IncompatibleVersion =>
        {
            CoreError::AgentIncompatible(business.message.clone())
        }
        _ => CapabilityRouter::agent_error(error),
    }
}

fn route_error(method: &str, error: crate::services::android_backend::RouteError) -> CoreError {
    use crate::services::android_backend::RouteError;
    let core = CapabilityRouter::core_error(error.clone());
    let detail = match error {
        RouteError::AgentUnavailable { reason, .. } => format!(
            "Android Agent 未连接或能力未就绪（{reason}）：请先在设备页「安装并连接」Agent，\
             Zygisk 应用清单必须经 Agent 的 ZygiskProvider 获取"
        ),
        RouteError::ProviderUnavailable { reason, .. } => reason,
        RouteError::UnsupportedMethod(_) => format!("Agent 未实现 {method}，请重新安装 Agent"),
        RouteError::LegacyFallbackNotRegistered(_) => {
            format!("{method} 只允许由 Zygisk Framework 提供，禁止用 pm/Shell 结果伪装成功")
        }
        RouteError::Incompatible(reason) => reason,
        RouteError::MutatingFallbackForbidden(method) => method,
        RouteError::AgentFailure(inner) => inner.to_string(),
    };
    match core {
        CoreError::AgentUnavailable(_) | CoreError::AgentIncompatible(_) => {
            CoreError::AgentUnavailable(detail)
        }
        other => other,
    }
}

fn map_localized_result(result: PackageListLocalizedResult) -> ZygiskAppList {
    let device_locale = result
        .items
        .first()
        .and_then(|item| item.resolved_locale.clone());
    // 旧版 Agent 不带 channel：宁可报「未知通道」，也不默认成能力更强的 v2
    let channel = result
        .channel
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zygisk_none".to_owned());
    ZygiskAppList {
        items: result
            .items
            .into_iter()
            .map(|item| ZygiskAppItem {
                package_name: item.package_name,
                label: item.label,
                version_name: item.version_name.unwrap_or_default(),
                version_code: item.version_code,
                label_source: label_source_name(item.label_source).to_owned(),
                requested_locale: item.requested_locale,
                resolved_locale: item.resolved_locale,
                fallback_reason: item.fallback_reason,
                uid: item.uid,
                is_system: item.is_system,
                enabled: item.enabled,
            })
            .collect(),
        success_count: result.success_count,
        fallback_count: result.fallback_count,
        warnings: result
            .warnings
            .into_iter()
            .map(|warning| ZygiskAppWarning {
                package_name: warning.package_name,
                code: warning.code,
                message: warning.message,
            })
            .collect(),
        device_locale,
        channel,
    }
}

/// 产物里缺了哪些分片。
///
/// 两个来源合并成一件事实：①模块明说跳过的（`too_large`）；②设备侧声明了、
/// 但这次没取回来的。合成单个 `.apks` 之后，"少一个分包"从肉眼可见变成不可见，
/// 所以宁可报"不完整"也不能默认成功。`declared` 为 `None`（老模块没有按包查询）
/// 时只信①，不猜②。
fn missing_parts(
    declared: Option<&[agent_protocol::DescribedApkFile]>,
    pulled: &[PulledPart],
    skipped: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = skipped.to_vec();
    if let Some(files) = declared {
        for file in files {
            if pulled.iter().any(|part| part.name == file.name) {
                continue;
            }
            // 模块已经用 `too_large` 报过名的，不再重复记一条
            if out.iter().any(|item| item.starts_with(&file.name)) {
                continue;
            }
            out.push(format!("{}（设备侧声明 {} 字节）", file.name, file.size));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn lifecycle_name(lifecycle: agent_protocol::ZygiskLifecycle) -> &'static str {
    use agent_protocol::ZygiskLifecycle as L;
    match lifecycle {
        L::NotInstalled => "not_installed",
        L::ZygiskDisabled => "zygisk_disabled",
        L::InstalledRebootRequired => "installed_reboot_required",
        L::Loaded => "loaded",
        L::BridgeReady => "bridge_ready",
        L::Incompatible => "incompatible",
        L::Faulted => "faulted",
    }
}

fn label_source_name(source: agent_protocol::LabelSource) -> &'static str {
    use agent_protocol::LabelSource as S;
    match source {
        S::Framework => "framework",
        S::Manifest => "manifest",
        S::PackageName => "package_name",
    }
}

fn validate_package_name(package_name: &str) -> CoreResult<()> {
    let ok = !package_name.is_empty()
        && package_name.len() <= 256
        && package_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::Internal("包名不合法".into()))
    }
}

/// 宿主侧装配用的临时目录：放在目标目录内，保证改名走同卷零拷贝。
fn stage_dir_for(destination: &Path, package_name: &str, session: &str) -> PathBuf {
    let tag: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    destination.join(format!(".apk-stage-{package_name}-{tag}"))
}

fn safe_filename(name: &str) -> CoreResult<String> {
    let path = Path::new(name);
    if name.is_empty()
        || name.len() > 200
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(CoreError::Internal(format!(
            "Agent 返回了不安全的文件名: {name}"
        )));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use agent_protocol::{LabelSource, LocalizedPackageItem, PackageWarning};

    use crate::services::apk_bundle;

    use super::*;

    #[test]
    fn local_path_inputs_are_confined_to_destination() {
        assert!(safe_filename("split_config.zh.apk").is_ok());
        assert!(safe_filename("../escape.apk").is_err());
        assert!(safe_filename("/tmp/evil.apk").is_err());
        assert!(safe_filename("sub/dir/base.apk").is_err());
        assert!(validate_package_name("com.example;rm -rf /").is_err());
        assert!(validate_package_name("com.example.app").is_ok());
    }

    #[test]
    fn localized_result_maps_to_camel_case_and_counts_fallbacks() {
        let result = PackageListLocalizedResult {
            items: vec![
                LocalizedPackageItem {
                    package_name: "com.amazon.mShop.android.shopping".into(),
                    label: "亚马逊购物".into(),
                    version_name: Some("32.17.0.100".into()),
                    version_code: Some(1243230206),
                    requested_locale: "zh-CN".into(),
                    resolved_locale: Some("zh-Hans-CN".into()),
                    label_source: LabelSource::Framework,
                    fallback_reason: None,
                    uid: Some(10233),
                    is_system: false,
                    enabled: true,
                },
                LocalizedPackageItem {
                    package_name: "com.google.android.overlay".into(),
                    label: "com.google.android.overlay".into(),
                    version_name: None,
                    version_code: None,
                    requested_locale: "zh-CN".into(),
                    resolved_locale: Some("zh-Hans-CN".into()),
                    label_source: LabelSource::PackageName,
                    fallback_reason: Some("framework_label_equals_package_name".into()),
                    uid: None,
                    is_system: true,
                    enabled: true,
                },
            ],
            success_count: 1,
            fallback_count: 1,
            warnings: vec![PackageWarning {
                package_name: None,
                code: "manifest_missing".into(),
                message: "1 个包缺少 E 清单".into(),
            }],
            channel: Some("zygisk_v2".into()),
        };

        let legacy = result.clone();
        let legacy_shape = {
            let mut copy = result.clone();
            copy.channel = None;
            copy
        };
        let list = map_localized_result(result);
        assert_eq!(list.device_locale.as_deref(), Some("zh-Hans-CN"));
        assert_eq!(list.success_count, 1);
        assert_eq!(list.fallback_count, 1);
        assert_eq!(list.warnings.len(), 1);

        let value = serde_json::to_value(&list).unwrap();
        assert_eq!(
            value["items"][0]["packageName"],
            "com.amazon.mShop.android.shopping"
        );
        assert_eq!(value["items"][0]["versionCode"], 1243230206_u64);
        assert_eq!(value["items"][0]["labelSource"], "framework");
        assert_eq!(value["items"][0]["isSystem"], false);
        assert_eq!(value["items"][1]["labelSource"], "package_name");
        assert_eq!(value["items"][1]["versionName"], "");
        assert_eq!(value["items"][1]["uid"], serde_json::Value::Null);
        assert_eq!(value["warnings"][0]["packageName"], serde_json::Value::Null);
        assert_eq!(value["channel"], "zygisk_v2");
        assert_eq!(map_localized_result(legacy_shape).channel, "zygisk_none");
        // 旧版 Agent 缺字段 -> 明确成 zygisk_none，绝不猜成 v2
        let mut legacy = legacy;
        legacy.channel = None;
        assert_eq!(map_localized_result(legacy).channel, "zygisk_none");
    }

    fn declared(names: &[(&str, u64)]) -> Option<Vec<agent_protocol::DescribedApkFile>> {
        Some(
            names
                .iter()
                .map(|(name, size)| agent_protocol::DescribedApkFile {
                    name: (*name).to_owned(),
                    size: *size,
                })
                .collect(),
        )
    }

    fn pulled_names(names: &[&str]) -> Vec<PulledPart> {
        names
            .iter()
            .map(|name| PulledPart {
                name: (*name).to_owned(),
                path: PathBuf::from(name),
                size: 1,
            })
            .collect()
    }

    #[test]
    fn missing_parts_merges_module_skips_and_undeclared_arrivals() {
        let all = declared(&[("base.apk", 10), ("split_config.arm64_v8a.apk", 20)]);

        // 齐件：一条都不报
        let parts = pulled_names(&["base.apk", "split_config.arm64_v8a.apk"]);
        assert!(missing_parts(all.as_deref(), &parts, &[]).is_empty());

        // 设备声明了却没取回来 -> 报缺，并带上声明尺寸
        let parts = pulled_names(&["base.apk"]);
        let missing = missing_parts(all.as_deref(), &parts, &[]);
        assert_eq!(missing.len(), 1);
        assert!(
            missing[0].starts_with("split_config.arm64_v8a.apk"),
            "{missing:?}"
        );

        // 模块已用 too_large 报过名的不重复记
        let parts = pulled_names(&["base.apk"]);
        let merged = missing_parts(
            all.as_deref(),
            &parts,
            &["split_config.arm64_v8a.apk".to_owned()],
        );
        assert_eq!(merged, vec!["split_config.arm64_v8a.apk".to_owned()]);

        // 老模块拿不到声明清单：只信模块明说的跳过，不凭空猜缺件
        let parts = pulled_names(&["base.apk"]);
        assert_eq!(
            missing_parts(None, &parts, &["x.apk".to_owned()]),
            vec!["x.apk".to_owned()]
        );
        assert!(missing_parts(None, &parts, &[]).is_empty());
    }

    #[test]
    fn describe_capability_is_not_faked_when_agent_is_missing() {
        // 与 list/export 同一条规矩：Zygisk 能力不许由 Legacy ADB 冒充
        // （本用例在 async 测试里跑，见 localized_list_refuses_legacy_fallback…）
        assert_eq!(agent_protocol::method::PACKAGE_DESCRIBE, "package.describe");
    }

    #[test]
    fn scope_maps_to_protocol_enum() {
        assert_eq!(LocalizedScope::User.as_protocol(), PackageScope::User);
        assert_eq!(LocalizedScope::System.as_protocol(), PackageScope::System);
        assert_eq!(LocalizedScope::All.as_protocol(), PackageScope::All);
        let parsed: LocalizedScope = serde_json::from_value(serde_json::json!("user")).unwrap();
        assert_eq!(parsed, LocalizedScope::User);
    }

    fn router(runner: Arc<dyn AdbRunner>) -> Arc<CapabilityRouter> {
        router_with_agent(runner).0
    }

    /// 真机测试需要先拿到 AgentManager 才能安装/启动 Agent，因此同时返回两者。
    fn router_with_agent(
        runner: Arc<dyn AdbRunner>,
    ) -> (
        Arc<CapabilityRouter>,
        Arc<crate::services::agent_manager::AgentManager>,
    ) {
        let config = Arc::new(crate::services::config_service::ConfigService::new(
            Arc::new(crate::db::Db::in_memory().unwrap()),
        ));
        let agent = Arc::new(crate::services::agent_manager::AgentManager::new(
            runner.clone(),
            Arc::new(crate::services::agent_artifact::AgentArtifactResolver::new(
                config, None,
            )),
        ));
        let router = Arc::new(CapabilityRouter::new(
            agent.clone(),
            runner,
            crate::services::android_backend::default_legacy_capabilities(),
        ));
        (router, agent)
    }

    #[tokio::test]
    async fn localized_list_refuses_legacy_fallback_when_agent_is_missing() {
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let service = ZygiskApplistService::new(router(runner.clone()), runner.clone());
        // Legacy ADB 没有登记 package.list_localized，缺 Agent 时不得回退成 pm 结果
        assert!(runner.environment().await.installed);

        let error = service
            .list("serial-a", None, LocalizedScope::All, false)
            .await
            .unwrap_err();
        assert!(
            matches!(error, CoreError::AgentUnavailable(_)),
            "缺 Agent 时必须显式失败而不是回退 pm: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("Agent"),
            "错误要能指导用户连接 Agent: {message}"
        );
        assert!(
            service
                .export_package("serial-a", "com.example.app", Path::new("/tmp"))
                .await
                .is_err()
        );
        // 新加的按包查询同一条规矩：没有 Agent 就是没有，不许用 pm 的结果冒充
        let error = service
            .describe("serial-a", "com.example.app")
            .await
            .unwrap_err();
        assert!(
            matches!(error, CoreError::AgentUnavailable(_)),
            "package.describe 也不许回退 Legacy: {error:?}"
        );
    }

    #[tokio::test]
    #[ignore = "需要已安装 applist 模块并重启生效的真机；APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_zygisk -- --ignored --nocapture"]
    async fn real_agent_zygisk_status_list_and_export() {
        use crate::services::device_service::RealAdbRunner;

        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(Arc::new(
            crate::services::config_service::ConfigService::new(Arc::new(
                crate::db::Db::in_memory().unwrap(),
            )),
        )));
        let (android, agent) = router_with_agent(runner.clone());
        let service = ZygiskApplistService::new(android, runner.clone());
        let status = agent
            .connect_resolved(&serial)
            .await
            .expect("Agent 安装/连接失败（真机测试需要先构建并推送 Agent 产物）");
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == PACKAGE_LIST_LOCALIZED),
            "Agent 未发布 package.list_localized capability: {:?}",
            status.capabilities
        );

        let status = service.status(&serial).await.unwrap();
        eprintln!(
            "[zygisk.status] lifecycle={} bridge={} root={} module={} impl={} locale={} latency={:?}ms",
            status.lifecycle,
            status.bridge_ready,
            status.root_available,
            status
                .module_version_code
                .map(|v| v.to_string())
                .unwrap_or_default(),
            status.zygisk_impl.clone().unwrap_or_default(),
            status.device_locale.clone().unwrap_or_default(),
            status.probe_latency_ms,
        );
        assert!(status.bridge_ready, "bridge 未就绪: {:?}", status.detail);
        assert_eq!(status.lifecycle, "bridge_ready");
        // applistpro 已安装时公共能力必须走 v2（指定 locale 与来源标注只有 v2 有）；
        // 未安装时才允许退回 demo v1，两种情况都要能自证。
        eprintln!(
            "[zygisk.channel] module={:?} sub_protocol={} version={:?} root={} detail={:?}",
            status.module_id,
            status.sub_protocol_version,
            status.module_version,
            status.root_available,
            status.detail
        );
        assert!(
            matches!(status.sub_protocol_version, 1 | 2),
            "子协议版本异常: {}",
            status.sub_protocol_version
        );
        if status.sub_protocol_version == 2 {
            assert_eq!(status.module_id.as_deref(), Some("applistpro"));
        } else {
            // v1 回退腿：必须显式说明这是 demo 通道、指定 locale 无证据可给
            assert_eq!(status.module_id.as_deref(), Some("applist"));
            let detail = status.detail.clone().unwrap_or_default();
            assert!(
                detail.contains("v1") || detail.contains("demo"),
                "退回 v1 时 detail 必须说明通道: {detail}"
            );
        }

        let user = service
            .list(&serial, Some("zh-CN".into()), LocalizedScope::User, false)
            .await
            .unwrap();
        assert!(!user.items.is_empty(), "三方应用清单为空");
        assert!(
            user.items.iter().all(|item| !item.is_system),
            "scope=user 混入了系统应用"
        );
        assert!(
            user.items
                .iter()
                .any(|item| item.label_source == "framework" && item.label != item.package_name),
            "没有 Framework 解析出的本地化名称"
        );
        assert!(
            user.items
                .windows(2)
                .all(|pair| pair[0].label.cmp(&pair[1].label) != std::cmp::Ordering::Greater),
            "清单必须按 label 稳定排序，否则 UI 刷新抖动"
        );

        if status.sub_protocol_version == 2 {
            let zh = service
                .list(&serial, Some("zh-CN".into()), LocalizedScope::All, false)
                .await
                .unwrap();
            let en = service
                .list(&serial, Some("en-US".into()), LocalizedScope::All, false)
                .await
                .unwrap();
            let zh_map: HashMap<&str, &crate::services::zygisk_applist::ZygiskAppItem> = zh
                .items
                .iter()
                .map(|i| (i.package_name.as_str(), i))
                .collect();
            let differing = en
                .items
                .iter()
                .filter(|item| {
                    zh_map
                        .get(item.package_name.as_str())
                        .is_some_and(|z| z.label != item.label)
                })
                .count();
            assert!(
                differing > 20,
                "v2 必须能按请求 locale 返回不同名称，实际只有 {differing} 个包不同"
            );
            assert!(
                en.items.iter().all(|item| item.requested_locale == "en-US"),
                "requested_locale 必须回显请求值"
            );
            // 诚实性：无跨语言证据的条目不得自称命中请求 locale
            let echo = en.items.iter().filter(|item| {
                item.resolved_locale.as_deref() == Some("en-US")
                    && zh_map
                        .get(item.package_name.as_str())
                        .is_some_and(|z| z.label == item.label)
            });
            assert_eq!(echo.count(), 0, "resolved_locale 出现回声");
            assert!(
                en.warnings.iter().any(|w| w.code == "locale_unproven"),
                "无证据条目必须汇总成 warning: {:?}",
                en.warnings
            );

            let with_disabled = service
                .list(&serial, None, LocalizedScope::All, true)
                .await
                .unwrap();
            assert!(
                with_disabled.items.len() >= zh.items.len(),
                "include_disabled 不得减少条目"
            );
            assert!(
                with_disabled.items.iter().any(|item| !item.enabled),
                "v2 应能枚举停用应用并标 enabled=false"
            );
        }

        let all = service
            .list(&serial, None, LocalizedScope::All, false)
            .await
            .unwrap();
        assert!(all.items.len() >= user.items.len());
        assert!(
            all.items.iter().any(|item| item.is_system),
            "scope=all 应包含系统应用"
        );
        let known = all
            .items
            .iter()
            .find(|item| item.label_source == "framework" && !item.version_name.is_empty());
        assert!(known.is_some(), "versionName 未从模块报文映射出来");

        // 指定一个设备默认 locale 之外的语言：不得假装按请求解析，必须给回退原因
        let en = service
            .list(&serial, Some("en-US".into()), LocalizedScope::User, false)
            .await
            .unwrap();
        if let Some(device) = en.device_locale.as_deref() {
            if !device.starts_with("en") {
                assert!(
                    en.warnings.iter().any(|w| w.code == "locale_not_honored"),
                    "locale 不匹配必须显式警告: {:?}",
                    en.warnings
                );
                assert!(en.items.iter().all(|item| item.requested_locale == "en-US"));
                assert!(
                    en.items
                        .iter()
                        .all(|item| item.resolved_locale.as_deref() == Some(device)),
                    "resolved_locale 必须是设备真实 locale，不能伪造"
                );
            }
        }

        // 默认样本是 cutout overlay（最小的单包系统组件，跑得快）；
        // APPLIST_EXPORT_PKG 可以换成任意应用来人工复核命名与产物类型。
        let wanted = std::env::var("APPLIST_EXPORT_PKG").unwrap_or_default();
        let target = all
            .items
            .iter()
            .find(|item| {
                if wanted.is_empty() {
                    item.package_name.ends_with(".cutout.emulation.noCutout")
                } else {
                    item.package_name == wanted
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "设备清单里没有 {}（默认样本是 cutout overlay 小包），无法做导出校验",
                    if wanted.is_empty() {
                        "cutout overlay 小包"
                    } else {
                        wanted.as_str()
                    }
                )
            });
        let out_dir = tempfile::tempdir().unwrap();
        let report = service
            .export_package(&serial, &target.package_name, out_dir.path())
            .await
            .unwrap();
        assert_eq!(report.package_name, target.package_name);
        assert!(!report.parts.is_empty());
        assert_eq!(
            report.bytes,
            report.parts.iter().map(|file| file.size).sum::<u64>()
        );
        assert!(report.complete, "模块跳过了分片: {:?}", report.skipped);
        // 命名元数据必须来自 Zygisk 清单，而不是界面回传或包名猜测
        assert!(
            report.name_source.starts_with("zygisk"),
            "命名来源异常: {}",
            report.name_source
        );
        // 命名走的哪条路由**模块能力**决定，而不是子协议版本号：v2.0 没有 P，
        // 就必须看见 zygisk_list_* 的降级路径（这是"旧模块 + 新 Desktop"的日常组合）；
        // v2.1+ 认识了 P，则必须看见单点查询与分包声明数。
        let module_has_describe = status
            .module_handlers
            .iter()
            .any(|handler| handler.cmd == "P" && handler.capability.as_deref() == Some("describe"));
        if module_has_describe {
            assert_eq!(
                report.name_source, "zygisk_describe",
                "模块认识 P 却没走按包查询，说明接线断了: {}",
                report.name_source
            );
            assert_eq!(report.declared_parts, Some(report.parts.len() as u64));
        } else {
            assert!(
                report.name_source.starts_with("zygisk_list"),
                "模块没有 P 时应退回批量清单并如实标注，实际: {}",
                report.name_source
            );
            assert_eq!(
                report.declared_parts, None,
                "退回清单这条路拿不到分包声明，不能假装校验过"
            );
        }
        assert_eq!(report.version_name, target.version_name);
        let naming = apk_bundle::AppNaming {
            package_name: report.package_name.clone(),
            label: report.app_label.clone(),
            version_name: report.version_name.clone(),
            version_code: target.version_code,
            name_source: report.name_source.clone(),
        };
        let stem = naming.stem();
        assert!(
            report.file_name.starts_with(&stem) && !stem.is_empty(),
            "产物名应以「Zygisk 显示名 + 版本号」开头: {:?} vs {:?}",
            report.file_name,
            stem
        );
        assert!(
            !report.file_name.contains('/') && !report.file_name.contains(".."),
            "产物名必须是单个安全的路径分量: {}",
            report.file_name
        );
        assert_eq!(
            report.kind,
            if report.parts.len() == 1 {
                "apk"
            } else {
                "apks"
            },
            "单包要出 .apk、分包要出 .apks"
        );
        let artifact = Path::new(&report.artifact_path);
        assert_eq!(artifact.parent(), Some(out_dir.path()));
        assert_eq!(
            artifact.file_name().and_then(|name| name.to_str()),
            Some(report.file_name.as_str())
        );
        assert_eq!(report.destination, out_dir.path().to_string_lossy());
        let bytes = std::fs::read(artifact).unwrap();
        assert_eq!(bytes.len() as u64, report.artifact_bytes);
        assert!(bytes.starts_with(b"PK"), "产物缺少 zip/APK 魔数");
        if report.kind == "apks" {
            let entries = apk_bundle::read_bundle(artifact).await.unwrap();
            // 条目顺序照 SAI 自己的写入器：两份 meta 在最前
            assert_eq!(entries[0].name, "meta.sai_v2.json");
            assert_eq!(entries[1].name, "meta.sai_v1.json");
            for part in &report.parts {
                assert!(
                    entries.iter().any(|entry| entry.name == part.name),
                    "容器里缺少 {}",
                    part.name
                );
            }
            for entry in &entries {
                let payload = apk_bundle::bundle_payload(artifact, entry).await.unwrap();
                assert_eq!(
                    apk_bundle::crc32(&payload),
                    entry.crc,
                    "{} CRC 不符",
                    entry.name
                );
            }
        } else {
            // 无分包：产物就是那份 APK 本身，改名不重写内容
            assert_eq!(bytes.len() as u64, report.parts[0].size);
            assert_eq!(report.file_name, format!("{stem}.apk"));
        }
        // 需要人工用第三方解压工具复核时，把产物留在指定目录（默认不留）
        if let Ok(keep) = std::env::var("APPLIST_EXPORT_KEEP_DIR") {
            tokio::fs::create_dir_all(&keep).await.unwrap();
            let held = Path::new(&keep).join(&report.file_name);
            tokio::fs::copy(Path::new(&report.artifact_path), &held)
                .await
                .unwrap();
            eprintln!(
                "[zygisk.export] {} -> {}（{} 个分片 / {} 字节，副本 {}）",
                report.package_name,
                report.file_name,
                report.parts.len(),
                report.bytes,
                held.display()
            );
        }
        // 宿主侧装配的临时目录必须回收，目标目录里只留产物
        let staged: Vec<String> = std::fs::read_dir(out_dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".apk-stage-"))
            .collect();
        assert!(staged.is_empty(), "装配暂存目录未清理: {staged:?}");

        // 可选腿：真·分包应用 -> 必须合并成一个 .apks。默认不跑（要拉几百 MB），
        // 需要时 APPLIST_EXPORT_SPLIT_PKG=com.amazon.mShop.android.shopping 显式开启。
        if let Ok(split_pkg) = std::env::var("APPLIST_EXPORT_SPLIT_PKG") {
            let item = all
                .items
                .iter()
                .find(|entry| entry.package_name == split_pkg)
                .unwrap_or_else(|| panic!("{split_pkg} 不在设备清单里"));
            let dir = tempfile::tempdir().unwrap();
            let merged = service
                .export_package(&serial, &item.package_name, dir.path())
                .await
                .unwrap();
            assert!(
                merged.parts.len() > 1,
                "{split_pkg} 只有一个 APK，本腿没有意义（改用单包应用会假绿）"
            );
            assert_eq!(merged.kind, "apks");
            assert!(merged.file_name.ends_with(".apks"));
            // 用户口径：名字与版本号之间用下划线，不用空格
            assert!(
                merged.file_name.contains('_'),
                "产物名要用下划线连接名字与版本号: {}",
                merged.file_name
            );
            assert!(
                merged.parts.iter().any(|part| part.name == "base.apk"),
                "分包应用必须有 base.apk: {:?}",
                merged.parts
            );
            let merged_path = Path::new(&merged.artifact_path);
            let entries = apk_bundle::read_bundle(merged_path).await.unwrap();
            assert_eq!(
                entries.len(),
                merged.parts.len() + 2,
                "容器里应是两份 SAI meta + 全部分包"
            );
            for part in &merged.parts {
                let entry = entries
                    .iter()
                    .find(|entry| entry.name == part.name)
                    .unwrap_or_else(|| panic!("容器里缺少 {}", part.name));
                assert_eq!(entry.size, part.size, "{} 大小不符", part.name);
                if part.name == "base.apk" {
                    let head = &apk_bundle::bundle_payload(merged_path, entry)
                        .await
                        .unwrap()[..4];
                    assert_eq!(&head[..2], b"PK", "base.apk 内容被改坏了");
                }
            }
            let meta: serde_json::Value = serde_json::from_slice(
                &apk_bundle::bundle_payload(merged_path, &entries[0])
                    .await
                    .unwrap(),
            )
            .unwrap();
            // SAI 的 v2 meta：字段名逐字对齐上游，"这些分包属于同一个应用"就靠它
            assert_eq!(meta["package"], split_pkg);
            assert_eq!(meta["meta_version"], 2);
            assert_eq!(meta["split_apk"], true);
            assert_eq!(meta["backup_components"][0]["type"], "apk_files");
            assert_eq!(meta["backup_components"][0]["size"], merged.bytes);
            let v1: serde_json::Value = serde_json::from_slice(
                &apk_bundle::bundle_payload(merged_path, &entries[1])
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(v1["package"], split_pkg);
            assert_eq!(v1["label"], meta["label"]);
            // 分包产物同样支持留一份人工复核副本（默认不留）
            if let Ok(keep) = std::env::var("APPLIST_EXPORT_KEEP_DIR") {
                tokio::fs::create_dir_all(&keep).await.unwrap();
                let held = Path::new(&keep).join(&merged.file_name);
                tokio::fs::copy(merged_path, &held).await.unwrap();
                eprintln!("[zygisk.apks] 复核副本: {}", held.display());
            }
            assert!(
                merged.artifact_bytes >= merged.bytes,
                "容器不该比原始分包更小"
            );
            eprintln!(
                "[zygisk.apks] {} = {} 个分包 / {} 字节 -> {}",
                split_pkg,
                merged.parts.len(),
                merged.bytes,
                merged.file_name
            );
        }
        // 设备侧暂存必须回收，避免残留 APK 副本
        let leftovers = runner
            .run(
                &runner.environment().await.path.unwrap(),
                &adb::build_args(
                    Some(&serial),
                    &adb::cmd_shell("ls /data/local/tmp | grep -c app-reverse-tools-apk || true"),
                ),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert_eq!(
            leftovers.stdout.trim(),
            "0",
            "导出暂存目录未清理: {}",
            leftovers.stdout
        );
        let forwards = runner
            .run(
                &runner.environment().await.path.unwrap(),
                &adb::build_args(Some(&serial), &adb::cmd_forward_list()),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert!(
            !forwards.stdout.contains(":11500"),
            "Desktop 不应再直连模块端口: {}",
            forwards.stdout
        );
    }

    /// AR10.3：按包查询（模块 `P`）与批量清单（模块 `L`）是两条独立实现，
    /// 同一个包在两边的显示名、版本号与分片集合必须逐字一致——不一致就说明
    /// 其中一条走了缓存或另一条解析漏了字段，导出命名就会与列表对不上。
    #[tokio::test]
    #[ignore = "需要已装 applistpro v2.1+ 的真机；APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_zygisk_describe -- --ignored --nocapture"]
    async fn real_agent_zygisk_describe_matches_list() {
        use crate::services::device_service::RealAdbRunner;

        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(Arc::new(
            crate::services::config_service::ConfigService::new(Arc::new(
                crate::db::Db::in_memory().unwrap(),
            )),
        )));
        let (android, agent) = router_with_agent(runner.clone());
        let service = ZygiskApplistService::new(android, runner.clone());
        agent
            .connect_resolved(&serial)
            .await
            .expect("Agent 安装/连接失败");
        let status = service.status(&serial).await.unwrap();
        assert!(status.bridge_ready, "bridge 未就绪: {:?}", status.detail);
        let has_describe = status
            .module_handlers
            .iter()
            .any(|handler| handler.cmd == "P" && handler.capability.as_deref() == Some("describe"));
        if !has_describe {
            eprintln!(
                "[跳过] 模块还不认识按包查询（需要 applistpro v2.1 以上并重启生效）：module={:?} version={:?} 注册表={:?}",
                status.module_id,
                status.module_version,
                status
                    .module_handlers
                    .iter()
                    .map(|handler| handler.cmd.clone())
                    .collect::<Vec<_>>()
            );
            return;
        }

        let list = service
            .list(&serial, None, LocalizedScope::All, true)
            .await
            .unwrap();
        // 抽样：三方 + 系统 + 一个已知分包应用（存在才抽），覆盖 labelSource 的几种取值
        let mut sample: Vec<ZygiskAppItem> = Vec::new();
        for pick in [
            list.items.iter().find(|item| !item.is_system),
            list.items.iter().find(|item| item.is_system),
            list.items
                .iter()
                .find(|item| item.label_source == "framework" && !item.version_name.is_empty()),
            list.items
                .iter()
                .find(|item| item.label_source == "package_name"),
        ]
        .into_iter()
        .flatten()
        {
            if !sample
                .iter()
                .any(|item| item.package_name == pick.package_name)
            {
                sample.push(pick.clone());
            }
        }
        assert!(!sample.is_empty(), "清单里抽不出样本");

        let mut framework_hits = 0;
        for item in &sample {
            let described = service.describe(&serial, &item.package_name).await.unwrap();
            assert_eq!(described.package_name, item.package_name);
            assert_eq!(
                described.label, item.label,
                "{} 的显示名两条路不一致",
                item.package_name
            );
            assert_eq!(
                described.version_name.as_deref().unwrap_or_default(),
                item.version_name,
                "{} 的 versionName 两条路不一致",
                item.package_name
            );
            assert_eq!(described.version_code, item.version_code);
            let source = match described.label_source {
                agent_protocol::LabelSource::Framework => "framework",
                agent_protocol::LabelSource::Manifest => "manifest",
                agent_protocol::LabelSource::PackageName => "package_name",
            };
            assert_eq!(source, item.label_source.as_str(), "名称来源两条路不一致");
            assert!(
                !described.apk_files.is_empty(),
                "{} 报出了零个 APK",
                item.package_name
            );
            // 分包应用才有 base.apk；单包应用的文件名可以随便（真机上就遇到过
            // NavigationBarMode3ButtonOverlay.apk 这种），所以只在多分时检查这条约定
            if described.apk_files.len() > 1 {
                assert!(
                    described
                        .apk_files
                        .iter()
                        .any(|file| file.name == "base.apk"),
                    "{} 有 {} 个分片却没有 base.apk: {:?}",
                    item.package_name,
                    described.apk_files.len(),
                    described.apk_files
                );
            }
            if described.label_source == agent_protocol::LabelSource::Framework {
                framework_hits += 1;
            }
            eprintln!(
                "[zygisk.describe] {} label={:?} version={:?} 分片={} device_locale={:?}",
                described.package_name,
                described.label,
                described.version_name,
                described.apk_files.len(),
                described.device_locale
            );
        }
        assert!(
            framework_hits > 0,
            "抽样里没有一个包解析出了 Framework 本地化名，说明按包查询可能退化成包名"
        );

        // 不存在的包：必须明确 NotFound，不能回一张空表也不能算能力故障
        let error = service
            .describe(&serial, "com.example.definitely.not.installed")
            .await
            .unwrap_err();
        eprintln!("[zygisk.describe] 不存在的包 -> {error}");
        assert!(
            error.to_string().contains("definitely"),
            "NotFound 要带包名，用户才知道自己在问哪个包: {error}"
        );

        // 导出：分片集合必须与按包查询声明的一致，产物名走 describe 这条路
        let target = &sample[0];
        let dir = tempfile::tempdir().unwrap();
        let report = service
            .export_package(&serial, &target.package_name, dir.path())
            .await
            .unwrap();
        let described = service
            .describe(&serial, &target.package_name)
            .await
            .unwrap();
        assert_eq!(report.name_source, "zygisk_describe");
        assert_eq!(
            report.declared_parts,
            Some(described.apk_files.len() as u64),
            "声明的分片数没被带进报告"
        );
        assert_eq!(report.parts.len(), described.apk_files.len());
        assert!(report.complete, "产物缺件: {:?}", report.skipped);
        assert_eq!(report.app_label, described.label);
    }

    /// AR5.6 / D021 第三腿：模块装着、v2 bridge 在监听，但 **adb shell 被撤销 root 授权**
    /// （KernelSU 里把 Shell 设为不允许），于是 Agent 读不到令牌。
    ///
    /// 这条腿的意义是证明「降级是显式的」：不能因为拿不到令牌就把 v2 当成可用，
    /// 也不能悄悄换成 v1 让用户以为指定 locale 生效了。前提不满足时明确跳过，
    /// 避免把「跑错状态」伪装成代码回归。
    #[tokio::test]
    #[ignore = "需要已装 applistpro 且已撤销 Shell root 的真机；APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_zygisk_v2_locked -- --ignored --nocapture"]
    async fn real_agent_zygisk_v2_locked_falls_back_to_v1_with_reason() {
        use crate::services::device_service::RealAdbRunner;

        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(Arc::new(
            crate::services::config_service::ConfigService::new(Arc::new(
                crate::db::Db::in_memory().unwrap(),
            )),
        )));
        let (android, agent) = router_with_agent(runner.clone());
        let service = ZygiskApplistService::new(android, runner.clone());
        agent
            .connect_resolved(&serial)
            .await
            .expect("Agent 安装/连接失败");
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();

        // 前提一：v2 模块确实在监听（否则这是「v2 没装」那条腿，不是本腿）
        let ports = runner
            .run(
                &adb_path,
                &adb::build_args(
                    Some(&serial),
                    &adb::cmd_shell("netstat -tln 2>/dev/null | grep -c 11501 || true"),
                ),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        // 前提二：Agent 拿不到 root（撤销 Shell 授权后成立）
        let status = service.status(&serial).await.unwrap();
        eprintln!(
            "[zygisk.v2locked] v2_port_listening={} root={} bridge={} sub_protocol={} module={:?} detail={:?}",
            ports.stdout.trim(),
            status.root_available,
            status.bridge_ready,
            status.sub_protocol_version,
            status.module_id,
            status.detail
        );
        if ports.stdout.trim() == "0" {
            eprintln!(
                "[跳过] 设备上没有 v2 bridge 在监听（11501），本腿需要已安装并启用 applistpro"
            );
            return;
        }
        if status.root_available {
            eprintln!("[跳过] Agent 仍有 root（请在 KernelSU 撤销 Shell 授权后重跑本腿）");
            return;
        }

        // root 撤销 + 端口活着 ⇒ 必须走 v1，且理由要同时说清「为什么不是 v2」和「v1 缺什么」
        assert!(
            status.bridge_ready,
            "v1 demo 通道应仍可用: {:?}",
            status.detail
        );
        assert_eq!(
            status.sub_protocol_version, 1,
            "读不到令牌却报了 v2，等于假装鉴权成功"
        );
        assert_eq!(status.module_id.as_deref(), Some("applist"));
        let detail = status.detail.clone().unwrap_or_default();
        assert!(
            detail.contains("令牌") || detail.contains("root"),
            "必须说明为何用不了 v2: {detail}"
        );
        assert!(
            detail.contains("v1") || detail.contains("demo"),
            "必须说明现在走的是 v1: {detail}"
        );
        assert!(
            detail.contains("指定 locale") && detail.contains("不可用"),
            "必须说明 v1 缺的能力: {detail}"
        );
        assert!(
            !detail.contains("  "),
            "文案里不应有整段空格残留: {detail:?}"
        );

        let list = service
            .list(&serial, Some("en-US".into()), LocalizedScope::User, false)
            .await
            .expect("v1 通道应仍能给出清单");
        assert_eq!(list.channel, "zygisk_v1", "通道必须如实标成 v1");
        assert!(!list.items.is_empty(), "v1 清单不应为空");
        assert!(
            list.items
                .iter()
                .all(|item| item.requested_locale == "en-US"),
            "requested_locale 仍要回显请求值，不能改写"
        );
        // v1 只能给设备默认语言的名字：resolved_locale 必须是设备真实 locale，
        // 绝不允许等于「请求的」locale（那等于假装按请求解析了）。
        let device = status.device_locale.clone().unwrap_or_default();
        assert!(!device.is_empty(), "status 应带回设备真实 locale");
        assert!(
            list.items
                .iter()
                .all(|item| item.resolved_locale.as_deref() == Some(device.as_str())),
            "v1 的 resolved_locale 只能是设备真实 locale {device}"
        );
        assert!(
            list.items
                .iter()
                .all(|item| item.resolved_locale.as_deref() != Some("en-US")),
            "请求 en-US 但走 v1，绝不能声称解析成了 en-US"
        );
        assert!(
            list.warnings.iter().any(|w| w.code == "locale_not_honored"),
            "v1 无法满足指定 locale，必须显式给 warning: {:?}",
            list.warnings
        );
    }

    /// AR5.5 的「Zygisk 缺失」真机腿：在没有 root、也没有任何模块的设备上，
    /// 诊断必须报「无通道」，清单必须显式失败并给可执行指引，绝不返回空列表冒充成功。
    #[tokio::test]
    #[ignore = "需要一台未安装 Zygisk 模块的真机；APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_zygisk_absent -- --ignored --nocapture"]
    async fn real_agent_zygisk_absent_reports_honest_failure() {
        use crate::services::device_service::RealAdbRunner;

        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(Arc::new(
            crate::services::config_service::ConfigService::new(Arc::new(
                crate::db::Db::in_memory().unwrap(),
            )),
        )));
        let (android, agent) = router_with_agent(runner.clone());
        let service = ZygiskApplistService::new(android, runner.clone());
        agent
            .connect_resolved(&serial)
            .await
            .expect("Agent 安装/连接失败（非 root 也应可用）");

        let status = service.status(&serial).await.unwrap();
        if status.bridge_ready || status.module_id.is_some() {
            // 这条腿的前提是「这台设备没装模块」。拿装了模块的机器跑它，
            // 失败信息会长得像代码回归，所以先自证前提再断言，不满足就明确跳过。
            eprintln!(
                "[跳过] 设备 {serial} 上已能观察到 Zygisk 模块（module={:?}, lifecycle={}），\
                 real_agent_zygisk_absent 需要一台未安装模块的设备（本项目用无模块的 vivo）",
                status.module_id, status.lifecycle
            );
            return;
        }
        eprintln!(
            "[zygisk.absent] lifecycle={} bridge={} root={} sub_protocol={} module={:?} detail={:?}",
            status.lifecycle,
            status.bridge_ready,
            status.root_available,
            status.sub_protocol_version,
            status.module_id,
            status.detail
        );
        assert!(!status.bridge_ready, "没有模块时不得声称 bridge 就绪");
        assert_eq!(status.sub_protocol_version, 0, "不得宣告任何子协议版本");
        assert_eq!(status.lifecycle, "faulted", "无法判定时必须如实报 faulted");
        let detail = status.detail.unwrap_or_default();
        assert!(
            detail.contains("无法区分") || detail.contains("未安装") || detail.contains("root"),
            "detail 要能告诉用户下一步查什么: {detail}"
        );

        let error = service
            .list(&serial, None, LocalizedScope::All, false)
            .await
            .expect_err("缺 Zygisk 时清单必须失败，不能返回空列表冒充成功");
        let message = error.to_string();
        assert!(
            message.contains("Zygisk") || message.contains("模块"),
            "错误必须指向模块安装/启用: {message}"
        );

        // 同一个 Agent 的普通包列表不依赖 root/Zygisk，必须照常可用
        let packages = service
            .plain_packages_for_test(&serial)
            .await
            .expect("package.list 不应依赖 Zygisk");
        assert!(!packages.is_empty(), "非 root 设备上普通包列表仍应可用");
    }

    #[test]
    fn status_serializes_lifecycle_as_stable_snake_case() {
        let status = ZygiskStatus {
            lifecycle: lifecycle_name(agent_protocol::ZygiskLifecycle::InstalledRebootRequired)
                .into(),
            bridge_ready: true,
            root_available: true,
            agent_connected: true,
            module_id: Some("applist".into()),
            module_version: Some("v1.0".into()),
            module_version_code: Some(1),
            zygisk_impl: Some("zygisksu".into()),
            device_locale: Some("zh-Hans-CN".into()),
            sub_protocol_version: 1,
            probe_latency_ms: Some(6),
            module_handlers: Vec::new(),
            detail: Some("模块有新版本待重启加载".into()),
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["lifecycle"], "installed_reboot_required");
        assert_eq!(value["bridgeReady"], true);
        assert_eq!(value["agentConnected"], true);
        assert_eq!(value["subProtocolVersion"], 1);
    }
}
