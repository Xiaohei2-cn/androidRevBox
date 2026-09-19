use std::collections::HashMap;
use std::process::Output;

use agent_protocol::method::{DEVICE_INFO, PACKAGE_LIST};
use agent_protocol::{
    AgentError, DeviceInfoParams, DeviceInfoResult, ErrorCode, PackageListParams,
    PackageListResult, PackageScope, PackageSummary, ProviderHealth, ProviderInfo,
};
use serde_json::{Value, to_value};
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const DEVICE_METHODS: &[&str] = &[DEVICE_INFO, PACKAGE_LIST];
const GETPROP: &str = "/system/bin/getprop";
const PM: &str = "/system/bin/pm";
const IP: &str = "/system/bin/ip";

pub struct DeviceProvider;

impl DeviceProvider {
    async fn device_info(&self, params: Value) -> Result<Value, AgentError> {
        let _: DeviceInfoParams = serde_json::from_value(params).map_err(|error| {
            AgentError::new(ErrorCode::InvalidRequest, "invalid device.info parameters")
                .with_details(serde_json::json!({ "reason": error.to_string() }))
        })?;
        let properties = checked_stdout(
            Command::new(GETPROP)
                .output()
                .await
                .map_err(|error| command_unavailable("getprop", error.to_string()))?,
            "getprop",
        )?;
        let properties = parse_getprop(&properties);
        let wlan_ipv4 = match Command::new(IP)
            .args(["-o", "-4", "addr", "show", "dev", "wlan0"])
            .output()
            .await
        {
            Ok(output) if output.status.success() => parse_wlan_ipv4(&output.stdout),
            _ => None,
        };
        serialize_result(device_info_from_properties(&properties, wlan_ipv4))
    }
}

impl DeviceProvider {
    /// `package.list`：在设备端解析 `pm list packages`，Desktop 不再拿文本自己切。
    /// 本地化显示名不归这里（必须走 Zygisk Provider），这里只给包名/uid/系统位/启用位。
    async fn package_list(&self, params: Value) -> Result<Value, AgentError> {
        let params: PackageListParams = serde_json::from_value(params).map_err(|error| {
            AgentError::new(ErrorCode::InvalidRequest, "invalid package.list parameters")
                .with_details(serde_json::json!({ "reason": error.to_string() }))
        })?;

        let mut items: Vec<PackageSummary> = Vec::new();
        for (flag, is_system) in [("-s", true), ("-3", false)] {
            if matches!(params.scope, PackageScope::User) && is_system {
                continue;
            }
            if matches!(params.scope, PackageScope::System) && !is_system {
                continue;
            }
            collect_packages(&mut items, &[flag, "-U", "-e"], is_system, true).await?;
            if params.include_disabled {
                collect_packages(&mut items, &[flag, "-U", "-d"], is_system, false).await?;
            }
            // 说明：`pm` 的第一个实参必须是子命令，直接传 `-3 -U -e` 会被当成命令名，
            // 设备侧回 255（真机首次运行即暴露，见 AR5.5 记录）。
        }
        items.sort_by(|left, right| left.package_name.cmp(&right.package_name));
        items.dedup_by(|left, right| left.package_name == right.package_name);
        serialize_result(PackageListResult { items })
    }
}

/// `args` 只是 `pm list packages` 之后的过滤参数（如 `-s -U -e`），子命令在此拼接。
async fn collect_packages(
    sink: &mut Vec<PackageSummary>,
    args: &[&str],
    is_system: bool,
    enabled: bool,
) -> Result<(), AgentError> {
    let mut argv: Vec<&str> = vec!["list", "packages"];
    argv.extend_from_slice(args);
    let output = Command::new(PM)
        .args(&argv)
        .output()
        .await
        .map_err(|error| command_unavailable("pm", error.to_string()))?;
    if !output.status.success() {
        return Err(
            AgentError::new(ErrorCode::ProviderUnavailable, "pm list packages failed")
                .with_details(serde_json::json!({
                    "args": argv,
                    "exit_code": output.status.code(),
                    "stderr": String::from_utf8_lossy(&output.stderr).trim().to_string(),
                })),
        );
    }
    for (package_name, uid) in parse_pm_uid_lines(&String::from_utf8_lossy(&output.stdout)) {
        sink.push(PackageSummary {
            package_name,
            uid,
            is_system,
            enabled,
        });
    }
    Ok(())
}

/// 解析 `pm list packages -U` 的行：`package:<pkg>[ uid:<n>]`。
/// uid 缺失时返回 `None`（旧 ROM/OEM 变异），不用 0 伪装成功。
pub(crate) fn parse_pm_uid_lines(text: &str) -> Vec<(String, Option<u32>)> {
    let mut rows = Vec::new();
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("package:") else {
            continue;
        };
        let (package, tail) = match rest.split_once(char::is_whitespace) {
            Some((package, tail)) => (package, Some(tail)),
            None => (rest, None),
        };
        // `package: uid:123` 这类畸形行：包名为空（切出来的首段其实是 uid:）必须跳过
        if package.is_empty() || package.starts_with("uid:") {
            continue;
        }
        let uid = tail
            .unwrap_or_default()
            .split_whitespace()
            .find_map(|token| {
                token
                    .strip_prefix("uid:")
                    .and_then(|value| value.parse::<u32>().ok())
            });
        rows.push((package.to_owned(), uid));
    }
    rows
}

impl Provider for DeviceProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "shell".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        DEVICE_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                DEVICE_INFO => self.device_info(params).await,
                PACKAGE_LIST => self.package_list(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported device method: {method}"),
                )),
            }
        })
    }
}

fn checked_stdout(output: Output, command: &str) -> Result<Vec<u8>, AgentError> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(AgentError::new(
        ErrorCode::ProviderUnavailable,
        format!("shell provider command failed: {command}"),
    )
    .with_details(serde_json::json!({ "exit_code": output.status.code() })))
}

fn command_unavailable(command: &str, reason: String) -> AgentError {
    AgentError::new(
        ErrorCode::ProviderUnavailable,
        format!("shell provider command unavailable: {command}"),
    )
    .with_details(serde_json::json!({ "reason": reason }))
}

fn parse_getprop(bytes: &[u8]) -> HashMap<String, String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let colon = line.find("]: [")?;
            let key = line.get(1..colon)?.trim();
            let value = line.get(colon + 4..)?.trim_end_matches(']').trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn device_info_from_properties(
    properties: &HashMap<String, String>,
    wlan_ipv4: Option<String>,
) -> DeviceInfoResult {
    let property = |key: &str| {
        properties
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    DeviceInfoResult {
        serial: property("ro.serialno"),
        model: property("ro.product.model"),
        manufacturer: property("ro.product.manufacturer"),
        android_version: property("ro.build.version.release"),
        api_level: property("ro.build.version.sdk").and_then(|value| value.parse().ok()),
        primary_abi: property("ro.product.cpu.abi"),
        wlan_ipv4,
    }
}

fn parse_wlan_ipv4(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut fields = text.split_whitespace();
    while let Some(field) = fields.next() {
        if field == "inet" {
            return fields
                .next()
                .and_then(|address| address.split('/').next())
                .filter(|address| !address.is_empty())
                .map(str::to_owned);
        }
    }
    None
}

fn serialize_result<T: serde::Serialize>(result: T) -> Result<Value, AgentError> {
    to_value(result).map_err(|error| {
        AgentError::new(ErrorCode::Internal, "failed to serialize provider result")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_properties_without_empty_sentinels() {
        let properties = parse_getprop(
            b"[ro.serialno]: [SERIAL-1]\n[ro.product.model]: [Pixel Test]\n[ro.product.manufacturer]: []\n[ro.build.version.release]: [16]\n[ro.build.version.sdk]: [36]\n[ro.product.cpu.abi]: [arm64-v8a]\n",
        );
        let result = device_info_from_properties(&properties, Some("192.0.2.4".into()));
        assert_eq!(result.serial.as_deref(), Some("SERIAL-1"));
        assert_eq!(result.model.as_deref(), Some("Pixel Test"));
        assert_eq!(result.manufacturer, None);
        assert_eq!(result.api_level, Some(36));
        assert_eq!(result.wlan_ipv4.as_deref(), Some("192.0.2.4"));
    }

    #[test]
    fn parses_pm_uid_lines_with_and_without_uid() {
        let rows = parse_pm_uid_lines(
            "package:com.a uid:10152\r\npackage:com.b\r\n\r\nnot-a-line\r\npackage: uid:5\r\n",
        );
        assert_eq!(
            rows,
            vec![
                ("com.a".to_owned(), Some(10152)),
                ("com.b".to_owned(), None),
            ],
            "空包名与非 package 行必须忽略，缺 uid 不得编 0"
        );
    }

    #[test]
    fn parses_wlan_ipv4_and_rejects_missing_inet_field() {
        assert_eq!(
            parse_wlan_ipv4(b"8: wlan0    inet 192.0.2.4/24 brd 192.0.2.255 scope global wlan0\n"),
            Some("192.0.2.4".into())
        );
        assert_eq!(parse_wlan_ipv4(b"8: wlan0: <NO-CARRIER>\n"), None);
    }
}
