use serde::{Deserialize, Serialize};

use crate::CapabilityInfo;

pub mod method {
    pub const SYSTEM_HELLO: &str = "system.hello";
    pub const SYSTEM_HEALTH: &str = "system.health";
    pub const CAPABILITY_LIST: &str = "capability.list";
    pub const DEVICE_INFO: &str = "device.info";
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_LIST_LOCALIZED: &str = "package.list_localized";
    pub const PACKAGE_EXPORT_APK: &str = "package.export_apk";
    pub const PACKAGE_EXPORT_CLEAN: &str = "package.export_clean";
    pub const ZYGISK_STATUS: &str = "zygisk.status";
}

/// Zygisk 模块生命周期。`installed_reboot_required` / `loaded` / `bridge_ready` 必须区分，
/// 「文件已装」不等于「接口可用」（AR5.3 契约）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZygiskLifecycle {
    NotInstalled,
    ZygiskDisabled,
    InstalledRebootRequired,
    Loaded,
    BridgeReady,
    Incompatible,
    Faulted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ZygiskStatusParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZygiskStatusResult {
    pub lifecycle: ZygiskLifecycle,
    pub bridge_ready: bool,
    /// 只表示「Agent 能读到模块目录」；root 不可用时为 false，不推断安装状态。
    pub root_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zygisk_impl: Option<String>,
    /// 设备默认 locale；模块按它解析 label，Agent 不伪造按请求 locale 的结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_locale: Option<String>,
    /// Agent <-> 模块私有子协议版本（当前冻结为 Q/E/D 线协议 = 1）。
    pub sub_protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportApkParams {
    pub package_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedApkFile {
    pub name: String,
    pub size: u64,
    /// 设备侧暂存路径，仅供 Desktop 走 ADB pull 传输使用。
    pub remote_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportApkResult {
    pub package_name: String,
    /// 一次性暂存会话标识，Desktop 取回后必须调用 `package.export_clean` 回收。
    pub session: String,
    pub files: Vec<StagedApkFile>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportCleanParams {
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportCleanResult {
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EmptyParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityListResult {
    pub capabilities: Vec<CapabilityInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceInfoParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfoResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_level: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_abi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wlan_ipv4: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageScope {
    All,
    User,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListParams {
    pub scope: PackageScope,
    pub include_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSummary {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub is_system: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListResult {
    pub items: Vec<PackageSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListLocalizedParams {
    /// `None` = 使用设备默认 locale（手机是中文就返回中文清单）；
    /// 指定 locale 且与设备默认不一致时，条目会带 `fallback_reason`，不冒充已按请求 locale 解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub scope: PackageScope,
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    Framework,
    Manifest,
    PackageName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizedPackageItem {
    pub package_name: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    pub requested_locale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_locale: Option<String>,
    pub label_source: LabelSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub is_system: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageWarning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListLocalizedResult {
    pub items: Vec<LocalizedPackageItem>,
    pub success_count: u32,
    pub fallback_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<PackageWarning>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn package_params_and_label_source_use_snake_case_values() {
        let params = PackageListLocalizedParams {
            locale: Some("zh-CN".into()),
            scope: PackageScope::User,
            include_disabled: true,
        };
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["scope"], "user");
        assert_eq!(
            serde_json::to_value(LabelSource::PackageName).unwrap(),
            "package_name"
        );
    }

    #[test]
    fn localized_result_preserves_fallback_and_omits_absent_optional_fields() {
        let result = PackageListLocalizedResult {
            items: vec![LocalizedPackageItem {
                package_name: "com.example.app".into(),
                label: "Example".into(),
                version_name: Some("1.2.3".into()),
                version_code: Some(45),
                requested_locale: "zh-CN".into(),
                resolved_locale: None,
                label_source: LabelSource::Manifest,
                fallback_reason: Some("no_zh_resource".into()),
                uid: None,
                is_system: false,
                enabled: true,
            }],
            success_count: 1,
            fallback_count: 1,
            warnings: vec![],
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["items"][0]["label_source"], "manifest");
        assert_eq!(value["items"][0]["fallback_reason"], "no_zh_resource");
        assert_eq!(value["items"][0].get("resolved_locale"), None);
        assert_eq!(value.get("warnings"), None);
        assert_eq!(value["success_count"], json!(1));
    }

    #[test]
    fn zygisk_status_lifecycle_is_snake_case_and_omits_absent_fields() {
        let result = ZygiskStatusResult {
            lifecycle: ZygiskLifecycle::InstalledRebootRequired,
            bridge_ready: false,
            root_available: true,
            module_id: Some("applist".into()),
            module_version: Some("v1.0".into()),
            module_version_code: Some(1),
            zygisk_impl: Some("zygisksu".into()),
            device_locale: Some("zh-Hans-CN".into()),
            sub_protocol_version: 1,
            probe_latency_ms: Some(4),
            detail: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["lifecycle"], "installed_reboot_required");
        assert_eq!(value["module_id"], "applist");
        assert_eq!(value["sub_protocol_version"], 1);
        assert_eq!(value.get("detail"), None);
    }

    #[test]
    fn localized_params_accept_missing_locale_as_device_default() {
        let params: PackageListLocalizedParams =
            serde_json::from_value(json!({ "scope": "all", "include_disabled": false })).unwrap();
        assert_eq!(params.locale, None);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("locale"), None);
    }

    #[test]
    fn staged_export_round_trips_files_and_session() {
        let result = PackageExportApkResult {
            package_name: "com.example.app".into(),
            session: "a1b2c3".into(),
            files: vec![StagedApkFile {
                name: "base.apk".into(),
                size: 1234,
                remote_path: "/data/local/tmp/x/base.apk".into(),
            }],
            bytes: 1234,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value["files"][0]["remote_path"],
            "/data/local/tmp/x/base.apk"
        );
        assert_eq!(value["files"][0]["name"], "base.apk");
    }

    #[test]
    fn device_info_optional_fields_do_not_use_empty_string_sentinels() {
        let result = DeviceInfoResult {
            serial: None,
            model: Some("Pixel Test".into()),
            manufacturer: None,
            android_version: Some("14".into()),
            api_level: Some(34),
            primary_abi: Some("arm64-v8a".into()),
            wlan_ipv4: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["model"], "Pixel Test");
        assert_eq!(value.get("manufacturer"), None);
        assert_eq!(value.get("wlan_ipv4"), None);
    }

    #[test]
    fn capability_list_uses_shared_capability_dto() {
        let result = CapabilityListResult {
            capabilities: vec![CapabilityInfo {
                method: method::SYSTEM_HEALTH.into(),
                version: 1,
                provider: "system".into(),
                available: true,
                unavailable_reason: None,
            }],
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["capabilities"][0]["method"], "system.health");
        assert_eq!(value["capabilities"][0]["provider"], "system");
    }
}
