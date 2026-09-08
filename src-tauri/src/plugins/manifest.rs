//! 插件 Manifest（plugin-sdk 契约 §5.3）：解析 + 校验。
//! 校验矩阵全部纯函数化，无 IO，单测覆盖（回测要求：合法/缺字段/ABI 不符/缺平台产物）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// host 支持的 ABI 版本（与 plugin_api.h 同步）
pub const HOST_ABI_VERSION: u32 = 1;

/// 插件类型白名单（总案 §5.1）
pub const PLUGIN_TYPES: [&str; 5] = ["device", "tool", "crypto", "parser", "workflow"];

/// 传输方式（P6 新增可选字段 `transport`；缺省 = in-process C ABI 动态库）。
/// process = 独立进程 + stdio JSON-RPC（协议见 docs/plugin-process-protocol.md），崩溃不拖垮主程序。
pub const TRANSPORT_IN_PROCESS: &str = "in-process";
pub const TRANSPORT_PROCESS: &str = "process";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub abi: u32,
    #[serde(rename = "type")]
    pub plugin_type: String,
    /// 平台键 → 相对插件目录的产物路径，如 "macos-arm64" -> "macos-arm64/libx.dylib"
    /// （process 插件：指向相对插件目录的可执行文件）
    pub entry: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// P6 可选：缺省视为 in-process；显式声明只允许这两个取值
    #[serde(default)]
    pub transport: Option<String>,
    /// P6 可选：平台键 → 产物 sha256（hex 小写）。安装时若声明则强校验，未声明跳过。
    #[serde(default)]
    pub integrity: BTreeMap<String, String>,
}

/// manifest 校验错误（前端展示文案在命令层拼装）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    EmptyField(&'static str),
    BadId(String),
    AbiMismatch { got: u32, want: u32 },
    UnknownType(String),
    UnknownTransport(String),
    MissingPlatformEntry(String),
    PathEscape,
}

impl std::error::Error for ManifestError {}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(k) => write!(f, "manifest 字段 {k} 为空"),
            Self::BadId(id) => write!(
                f,
                "插件 id 非法（需 [a-z0-9._-]，形如 crypto.base64）: {id}"
            ),
            Self::AbiMismatch { got, want } => {
                write!(f, "ABI 版本不兼容：插件要求 {got}，host 支持 {want}")
            }
            Self::UnknownType(t) => {
                write!(f, "未知插件类型: {t}（允许 {}）", PLUGIN_TYPES.join("/"))
            }
            Self::UnknownTransport(t) => write!(
                f,
                "未知 transport: {t}（允许 {TRANSPORT_IN_PROCESS}/{TRANSPORT_PROCESS}）"
            ),
            Self::MissingPlatformEntry(p) => write!(f, "manifest 缺少当前平台产物: {p}"),
            Self::PathEscape => write!(f, "manifest 产物路径越出插件目录"),
        }
    }
}

pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// 当前平台的 manifest entry 键。
/// 平台名 + 架构（std::env::consts::ARCH 的 x86_64 归一为 x64，与总案示例一致）。
pub fn current_platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    // 总案 §5.3 命名：macos-arm64 / windows-x64 / linux-x64
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

impl PluginManifest {
    /// 解析 manifest.json 文本（serde 报错直接以字符串返回）
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("manifest JSON 解析失败: {e}"))
    }

    /// 完整校验；platform_key 显式传入便于跨平台单测
    pub fn validate(&self, platform_key: &str) -> Result<(), ManifestError> {
        if self.id.trim().is_empty() {
            return Err(ManifestError::EmptyField("id"));
        }
        if !is_valid_id(&self.id) {
            return Err(ManifestError::BadId(self.id.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::EmptyField("name"));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::EmptyField("version"));
        }
        if self.abi != HOST_ABI_VERSION {
            return Err(ManifestError::AbiMismatch {
                got: self.abi,
                want: HOST_ABI_VERSION,
            });
        }
        if !PLUGIN_TYPES.contains(&self.plugin_type.as_str()) {
            return Err(ManifestError::UnknownType(self.plugin_type.clone()));
        }
        if let Some(t) = &self.transport {
            if t != TRANSPORT_IN_PROCESS && t != TRANSPORT_PROCESS {
                return Err(ManifestError::UnknownTransport(t.clone()));
            }
        }
        let entry = self
            .entry
            .get(platform_key)
            .ok_or_else(|| ManifestError::MissingPlatformEntry(platform_key.to_string()))?;
        // 产物路径禁止绝对路径与 `..`（加载时还会二次 canonicalize 防符号链接逃逸）
        let path = std::path::Path::new(entry);
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ManifestError::PathEscape);
        }
        Ok(())
    }

    pub fn entry_for(&self, platform_key: &str) -> Option<&str> {
        self.entry.get(platform_key).map(String::as_str)
    }

    /// 传输方式；未声明字段时视为 in-process
    pub fn transport(&self) -> &'static str {
        match self.transport.as_deref() {
            Some(TRANSPORT_PROCESS) => TRANSPORT_PROCESS,
            _ => TRANSPORT_IN_PROCESS,
        }
    }

    pub fn is_process(&self) -> bool {
        self.transport() == TRANSPORT_PROCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> PluginManifest {
        let mut entry = BTreeMap::new();
        entry.insert(
            "macos-arm64".to_string(),
            "macos-arm64/libx.dylib".to_string(),
        );
        entry.insert("windows-x64".to_string(), "windows-x64/x.dll".to_string());
        PluginManifest {
            id: "crypto.base64".into(),
            name: "Base64".into(),
            version: "1.0.0".into(),
            abi: 1,
            plugin_type: "crypto".into(),
            entry,
            capabilities: vec!["encode".into(), "decode".into()],
            transport: None,
            integrity: BTreeMap::new(),
        }
    }

    #[test]
    fn accepts_valid_manifest() {
        assert_eq!(valid().validate("macos-arm64"), Ok(()));
    }

    #[test]
    fn rejects_zero_id() {
        let mut m = valid();
        m.id = "".into();
        assert_eq!(
            m.validate("macos-arm64"),
            Err(ManifestError::EmptyField("id"))
        );
    }

    #[test]
    fn rejects_bad_id_charset() {
        let mut m = valid();
        m.id = "Crypto Base!".into();
        assert!(matches!(
            m.validate("macos-arm64"),
            Err(ManifestError::BadId(_))
        ));
    }

    #[test]
    fn rejects_abi_mismatch() {
        let mut m = valid();
        m.abi = 2;
        assert_eq!(
            m.validate("macos-arm64"),
            Err(ManifestError::AbiMismatch { got: 2, want: 1 })
        );
    }

    #[test]
    fn rejects_unknown_type() {
        let mut m = valid();
        m.plugin_type = "magic".into();
        assert!(matches!(
            m.validate("macos-arm64"),
            Err(ManifestError::UnknownType(_))
        ));
    }

    #[test]
    fn rejects_missing_platform_entry() {
        let mut m = valid();
        m.entry.remove("macos-arm64");
        assert_eq!(
            m.validate("macos-arm64"),
            Err(ManifestError::MissingPlatformEntry("macos-arm64".into()))
        );
    }

    #[test]
    fn rejects_path_escape_in_entry() {
        let mut m = valid();
        m.entry.insert("macos-arm64".into(), "../evil.dylib".into());
        assert_eq!(m.validate("macos-arm64"), Err(ManifestError::PathEscape));
        let mut abs = valid();
        abs.entry.insert("macos-arm64".into(), "/etc/passwd".into());
        assert_eq!(abs.validate("macos-arm64"), Err(ManifestError::PathEscape));
    }

    #[test]
    fn parses_total_plan_sm4_manifest() {
        // 总案 §5.3 的示例 manifest 必须能通过（entry 键为总案命名）
        let text = r#"{
          "id": "crypto.sm4",
          "name": "SM4",
          "version": "1.0.0",
          "abi": 1,
          "type": "crypto",
          "entry": {
            "macos-arm64": "macos-arm64/libcrypto_sm4.dylib",
            "windows-x64": "windows-x64/crypto_sm4.dll",
            "linux-x64": "linux-x64/libcrypto_sm4.so"
          },
          "capabilities": ["encrypt", "decrypt"]
        }"#;
        let m = PluginManifest::parse(text).unwrap();
        assert_eq!(m.validate("macos-arm64"), Ok(()));
        assert_eq!(
            m.entry_for("windows-x64"),
            Some("windows-x64/crypto_sm4.dll")
        );
    }

    #[test]
    fn transport_defaults_to_in_process_and_accepts_process() {
        let m = valid();
        assert_eq!(m.transport(), TRANSPORT_IN_PROCESS);
        assert!(!m.is_process());

        let mut p = valid();
        p.transport = Some(TRANSPORT_PROCESS.into());
        assert_eq!(p.validate("macos-arm64"), Ok(()));
        assert!(p.is_process());
    }

    #[test]
    fn rejects_unknown_transport() {
        let mut m = valid();
        m.transport = Some("rpc-over-carrier-pigeon".into());
        assert_eq!(
            m.validate("macos-arm64"),
            Err(ManifestError::UnknownTransport(
                "rpc-over-carrier-pigeon".into()
            ))
        );
    }

    #[test]
    fn parses_p6_optional_fields() {
        // P4 时期无 transport/integrity 字段的 manifest 必须继续可用（只加不改）；
        // P6 新字段可缺省，也可显式声明
        let text = r#"{
          "id": "tool.echo",
          "name": "Echo",
          "version": "0.2.0",
          "abi": 1,
          "type": "tool",
          "entry": { "macos-arm64": "macos-arm64/echo" },
          "transport": "process",
          "integrity": { "macos-arm64": "abc123" }
        }"#;
        let m = PluginManifest::parse(text).unwrap();
        assert_eq!(m.validate("macos-arm64"), Ok(()));
        assert!(m.is_process());
        assert_eq!(
            m.integrity.get("macos-arm64").map(String::as_str),
            Some("abc123")
        );
    }

    #[test]
    fn parse_error_is_readable() {
        let e = PluginManifest::parse("{ oops").unwrap_err();
        assert!(e.contains("manifest JSON"), "{e}");
    }
}
