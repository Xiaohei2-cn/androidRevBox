//! Zygisk 应用清单 Service（AR5.3/AR5.4）。
//!
//! 边界：模块的 `Q/E/D` 私有线协议由 **Android Agent 的 ZygiskProvider** 负责，
//! Desktop 只调用 Agent typed API，不再直连模块端口；APK 字节回传复用
//! 「Desktop Transport」允许项（ADB pull），因此本文件不出现任何 Zygisk 私有协议。
//! 该能力不允许静默降级成 `pm`/Shell 结果（AR5.5 规则）：缺 Agent 或缺模块时
//! 直接返回可诊断的 `provider_unavailable`。

use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Duration;

use agent_protocol::method::{
    PACKAGE_EXPORT_APK, PACKAGE_EXPORT_CLEAN, PACKAGE_LIST_LOCALIZED, ZYGISK_STATUS,
};
use agent_protocol::{
    PackageExportApkParams, PackageExportApkResult, PackageExportCleanParams,
    PackageListLocalizedParams, PackageListLocalizedResult, PackageScope, ZygiskStatusResult,
};
use serde::{Deserialize, Serialize};

use crate::adapters::adb;
use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{AgentSessionState, AndroidBackendSource};
use crate::services::android_backend::{AgentBackendError, CapabilityRouter, OperationKind};
use crate::services::device_service::AdbRunner;

const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(900);
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
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskApkFile {
    pub package_name: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskExportReport {
    pub package_name: String,
    pub files: Vec<ZygiskApkFile>,
    pub destination: String,
    pub bytes: u64,
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

    /// 导出 base + split APK：Agent 在设备侧暂存，Desktop 经 ADB pull 取回并校验大小。
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

        let package_dir = destination.join(package_name);
        tokio::fs::create_dir_all(&package_dir)
            .await
            .map_err(|error| CoreError::Internal(format!("创建导出目录失败: {error}")))?;

        let pull = self.pull_staged_files(serial, &staged, &package_dir).await;
        // 无论取回成功与否都回收设备侧暂存，避免残留 APK 副本。
        if let Err(error) = self.clean_staged(serial, &staged.session).await {
            tracing::warn!(serial, error = %error, "Zygisk 导出暂存清理失败");
        }
        let files = pull?;

        let bytes = files.iter().map(|file| file.size).sum::<u64>();
        Ok(ZygiskExportReport {
            package_name: staged.package_name,
            files,
            destination: package_dir.to_string_lossy().into_owned(),
            bytes,
        })
    }

    async fn pull_staged_files(
        &self,
        serial: &str,
        staged: &PackageExportApkResult,
        package_dir: &Path,
    ) -> CoreResult<Vec<ZygiskApkFile>> {
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
            files.push(ZygiskApkFile {
                package_name: staged.package_name.clone(),
                name,
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
            "[zygisk.channel] module={:?} sub_protocol={} version={:?}",
            status.module_id, status.sub_protocol_version, status.module_version
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

        let target = all
            .items
            .iter()
            .find(|item| item.package_name.ends_with(".cutout.emulation.noCutout"))
            .expect("样本设备缺少 cutout overlay 小包，无法做导出体积校验");
        let out_dir = tempfile::tempdir().unwrap();
        let report = service
            .export_package(&serial, &target.package_name, out_dir.path())
            .await
            .unwrap();
        assert_eq!(report.package_name, target.package_name);
        assert!(!report.files.is_empty());
        assert_eq!(
            report.bytes,
            report.files.iter().map(|file| file.size).sum::<u64>()
        );
        for file in &report.files {
            let bytes = std::fs::read(Path::new(&report.destination).join(&file.name)).unwrap();
            assert_eq!(bytes.len() as u64, file.size, "{} 大小不符", file.name);
            assert!(bytes.starts_with(b"PK"), "{} 缺少 zip 魔数", file.name);
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
            detail: Some("模块有新版本待重启加载".into()),
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["lifecycle"], "installed_reboot_required");
        assert_eq!(value["bridgeReady"], true);
        assert_eq!(value["agentConnected"], true);
        assert_eq!(value["subProtocolVersion"], 1);
    }
}
