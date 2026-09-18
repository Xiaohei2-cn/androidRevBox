use serde::{Deserialize, Serialize};

use crate::CapabilityInfo;

pub mod method {
    pub const SYSTEM_HELLO: &str = "system.hello";
    pub const SYSTEM_HEALTH: &str = "system.health";
    pub const CAPABILITY_LIST: &str = "capability.list";
    pub const DEVICE_INFO: &str = "device.info";
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_LIST_LOCALIZED: &str = "package.list_localized";
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
    pub locale: String,
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
            locale: "zh-CN".into(),
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
