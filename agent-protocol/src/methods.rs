use serde::{Deserialize, Serialize};

use crate::CapabilityInfo;

pub mod method {
    pub const SYSTEM_HELLO: &str = "system.hello";
    pub const SYSTEM_HEALTH: &str = "system.health";
    pub const CAPABILITY_LIST: &str = "capability.list";
    pub const DEVICE_INFO: &str = "device.info";
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_LIST_LOCALIZED: &str = "package.list_localized";
    pub const ACTIVITY_FOREGROUND: &str = "activity.foreground";
    pub const PROCESS_PORTS: &str = "process.ports";
    pub const PROCESS_BY_PORT: &str = "process.by_port";
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

/// 前台应用（AR6.1）：`dumpsys window`/`pidof`/`/proc` 的解析全部在设备端完成，
/// Desktop 不再拼 shell 字符串。读不到的字段保持 `None`，不用空串或 0 伪装成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    ThirdParty,
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActivityForegroundParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcEntrySummary {
    /// maps | cmdline | status
    pub name: String,
    pub path: String,
    /// maps=行数、cmdline=命令行（截断）、status=头几行；不可读为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityForegroundResult {
    /// false = 未解析到前台窗口（锁屏、弹窗或 ROM 输出差异），不算错误
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub package_kind: PackageKind,
    /// `legacyNativeLibraryDir`：部分 ROM 不再输出该字段，缺失即 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lib_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proc: Vec<ProcEntrySummary>,
    /// 无前台时的原因提示，便于 UI 直接展示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// AR6.2：端口/进程互查。`/proc/net/*` 解析与 fd→inode 匹配全部在设备端完成，
/// Desktop 不再 cat 全文 + 宿主解析，也不再分批 `ls /proc/*/fd`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessPortsParams {
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningPort {
    pub port: u16,
    /// `/proc/net` 里十六进制地址还原后的可读形式（IPv4 点分 / IPv6 冒分）
    pub address: String,
    pub family: SocketFamily,
    /// `listen`/`time_wait`/... 已按内核 st 值翻译；未收录值保留 `st=<hex>`
    pub state: String,
    pub inode: u64,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPortsResult {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmdline: Option<String>,
    pub ports: Vec<ListeningPort>,
    /// 读不到就列出来（权限不足或进程已退出），不静默当成"没有监听端口"
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketFamily {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessByPortParams {
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortHoldingProcess {
    pub pid: u32,
    pub uid: u32,
    pub family: SocketFamily,
    pub address: String,
    pub state: String,
    pub inode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessByPortResult {
    pub port: u16,
    pub sockets: Vec<PortHoldingProcess>,
    /// 无法确定属主的 socket（未找到持有该 inode 的进程）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unowned: Vec<PortHoldingProcess>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// 候选进程数超过扫描上限时列出被跳过的原因，方便判断"查不到"是权限还是上限
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

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
    /// 实际服务本次清单的通道，UI 必须可见：`zygisk_v2`（可指定 locale）、
    /// `zygisk_v1`（demo 模块，只有设备默认 locale）或 `zygisk_none`。
    /// 缺字段表示旧版 Agent，前端按 `zygisk_none` 处理，不猜成 v2。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
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
            channel: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["items"][0]["label_source"], "manifest");
        assert_eq!(value["items"][0]["fallback_reason"], "no_zh_resource");
        assert_eq!(value["items"][0].get("resolved_locale"), None);
        assert_eq!(value.get("warnings"), None);
        assert_eq!(value["success_count"], json!(1));
    }

    #[test]
    fn foreground_result_is_snake_case_and_omits_absent_fields() {
        let result = ActivityForegroundResult {
            found: true,
            package_name: Some("com.target.app".into()),
            activity: Some("com.target.app.ui.HomeActivity".into()),
            pid: Some(4321),
            package_kind: PackageKind::ThirdParty,
            native_lib_dir: None,
            proc: vec![ProcEntrySummary {
                name: "maps".into(),
                path: "/proc/4321/maps".into(),
                summary: Some("118".into()),
                readable: true,
            }],
            hint: None,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["package_name"], "com.target.app");
        assert_eq!(value["package_kind"], "third_party");
        assert_eq!(value["proc"][0]["readable"], true);
        assert_eq!(value.get("native_lib_dir"), None);
        assert_eq!(value.get("hint"), None);

        let empty = ActivityForegroundResult {
            found: false,
            package_name: None,
            activity: None,
            pid: None,
            package_kind: PackageKind::Unknown,
            native_lib_dir: None,
            proc: Vec::new(),
            hint: Some("未解析到前台窗口".into()),
        };
        let value = serde_json::to_value(empty).unwrap();
        assert_eq!(value["package_kind"], "unknown");
        assert_eq!(value.get("proc"), None);
        let parsed: ActivityForegroundResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.package_kind, PackageKind::Unknown);
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
    fn process_ports_result_distinguishes_empty_from_unreadable() {
        let result = ProcessPortsResult {
            pid: 4321,
            comm: Some("com.target.app".into()),
            cmdline: None,
            ports: vec![ListeningPort {
                port: 11501,
                address: "127.0.0.1".into(),
                family: SocketFamily::Ipv4,
                state: "listen".into(),
                inode: 4242,
                uid: 0,
            }],
            unreadable: vec!["/proc/net/tcp6".into()],
            truncated: false,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["ports"][0]["family"], "ipv4");
        assert_eq!(value["ports"][0]["port"], 11501);
        assert_eq!(value.get("cmdline"), None);
        assert_eq!(value.get("truncated"), None);
        assert_eq!(value["unreadable"][0], "/proc/net/tcp6");
        let parsed: ProcessPortsResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.ports[0].family, SocketFamily::Ipv4);
    }

    #[test]
    fn process_by_port_result_splits_owned_and_unowned_sockets() {
        let socket = PortHoldingProcess {
            pid: 0,
            uid: 0,
            family: SocketFamily::Ipv6,
            address: "::".into(),
            state: "listen".into(),
            inode: 99,
            comm: None,
        };
        let result = ProcessByPortResult {
            port: 8081,
            sockets: vec![socket.clone()],
            unowned: vec![socket],
            truncated: true,
            skipped: vec!["scanned_pid_limit=4000".into()],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["sockets"][0]["family"], "ipv6");
        assert_eq!(value["sockets"][0].get("comm"), None);
        assert_eq!(value["truncated"], json!(true));
        assert_eq!(value["skipped"][0], "scanned_pid_limit=4000");
        // pid=0 表示"socket 存在但属主未知"，不能与"没有该端口"混淆
        let empty: ProcessByPortResult =
            serde_json::from_value(json!({"port": 1, "sockets": []})).unwrap();
        assert!(empty.sockets.is_empty() && empty.unowned.is_empty());
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
    fn localized_channel_is_optional_and_snake_case_compatible() {
        let mut result = PackageListLocalizedResult {
            items: Vec::new(),
            success_count: 0,
            fallback_count: 0,
            warnings: Vec::new(),
            channel: Some("zygisk_v2".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["channel"], "zygisk_v2");
        assert_eq!(value.get("warnings"), None);
        result.channel = None;
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value.get("channel"), None);
        // 旧版 Agent 报文没有该字段，也必须能解出来（不得当成 v2）
        let legacy: PackageListLocalizedResult =
            serde_json::from_value(json!({"items": [], "success_count": 0, "fallback_count": 0}))
                .unwrap();
        assert_eq!(legacy.channel, None);
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
