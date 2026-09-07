//! crypto-base64：示例插件，演示用 at-plugin-sdk 实现 C ABI v1。
//!
//! payload 协议（UTF-8 JSON，业务错误走 payload 而非 C 错误码）：
//!   请求  {"op":"encode"|"decode", "data":"..."}
//!   成功  {"ok":true, "data":"..."}
//!   失败  {"ok":false, "code":<正整数>, "err":"..."}
//! 错误码：1=非法 JSON，2=未知 op，3=base64 解码失败。
//! C ABI 返回值仅承载框架级错误（SDK 保证：panic=-99，参数非法=-1）。

use at_plugin_sdk::{AtPlugin, PluginDescriptor, export_plugin};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

pub struct Base64Plugin;

fn fail(code: i32, err: &str) -> Vec<u8> {
    serde_json::json!({"ok": false, "code": code, "err": err})
        .to_string()
        .into_bytes()
}

impl AtPlugin for Base64Plugin {
    const META: PluginDescriptor = PluginDescriptor {
        id: "crypto.base64",
        name: "Base64",
        version: "1.0.0",
        plugin_type: "crypto",
    };

    fn call(input: &[u8]) -> Result<Vec<u8>, i32> {
        let Ok(req) = serde_json::from_slice::<serde_json::Value>(input) else {
            return Ok(fail(1, "invalid json input"));
        };
        let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let data = req.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let body = match op {
            "encode" => {
                serde_json::json!({"ok": true, "data": STANDARD.encode(data)})
            }
            "decode" => match STANDARD.decode(data) {
                Ok(bytes) => serde_json::json!({
                    "ok": true,
                    "data": String::from_utf8_lossy(&bytes),
                }),
                Err(_) => serde_json::json!({
                    "ok": false, "code": 3, "err": "invalid base64",
                }),
            },
            _ => serde_json::json!({
                "ok": false, "code": 2, "err": "unknown op",
            }),
        };
        Ok(body.to_string().into_bytes())
    }
}

export_plugin!(Base64Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    fn call(req: serde_json::Value) -> serde_json::Value {
        let out = Base64Plugin::call(req.to_string().as_bytes()).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn encode_roundtrip() {
        let enc = call(serde_json::json!({"op":"encode","data":"hello 世界"}));
        assert_eq!(enc["ok"], true);
        let b64 = enc["data"].as_str().unwrap();
        let dec = call(serde_json::json!({"op":"decode","data":b64}));
        assert_eq!(dec["data"], "hello 世界");
    }

    #[test]
    fn invalid_base64_fails_with_code3() {
        let dec = call(serde_json::json!({"op":"decode","data":"!!!"}));
        assert_eq!(dec["ok"], false);
        assert_eq!(dec["code"], 3);
    }

    #[test]
    fn unknown_op_code2_and_bad_json_code1() {
        let r = call(serde_json::json!({"op":"md5","data":"x"}));
        assert_eq!(r["code"], 2);
        let raw = Base64Plugin::call(b"not json".as_slice()).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["code"], 1);
    }
}
