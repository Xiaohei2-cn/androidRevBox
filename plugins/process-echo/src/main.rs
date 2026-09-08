//! process-echo：Process Plugin 协议（docs/plugin-process-protocol.md）的参考实现与测试夹具。
//!
//! stdio 行分隔 JSON-RPC 2.0：
//!   ← {"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1}}
//!   → {"jsonrpc":"2.0","id":0,"result":{"id":"tool.echo","name":"Echo","version":"0.2.0","type":"tool"}}
//!   ← {"jsonrpc":"2.0","id":N,"method":"call","params":{"input":"<json 文本>"}}
//!   → {"jsonrpc":"2.0","id":N,"result":{"code":0,"output":"<json 文本>"}}
//!   ← {"jsonrpc":"2.0","method":"shutdown"}（通知，收到后立即退出）
//!
//! call 的 op 集：echo（回显）/ upper（转大写）/ crash（进程立即退出，验证崩溃隔离）/
//! sleep_ms（睡眠，验证超时终止）。业务错误走 output 内 {"ok":false,...}，与 crypto-base64 一致。

use std::io::{BufRead, Write};

fn respond(out: &mut impl Write, id: u64, result: serde_json::Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":id,"result":result})
    );
    let _ = out.flush();
}

fn handle_call(input: &str) -> serde_json::Value {
    let req: serde_json::Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => {
            return serde_json::json!({"code": 1, "output": "{\"ok\":false,\"code\":1,\"err\":\"invalid json\"}"});
        }
    };
    let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");
    match op {
        "echo" => serde_json::json!({
            "code": 0,
            "output": serde_json::json!({"ok": true, "data": req.get("data").cloned().unwrap_or(serde_json::Value::Null)}).to_string()
        }),
        "upper" => serde_json::json!({
            "code": 0,
            "output": serde_json::json!({"ok": true, "data": req.get("data").and_then(|v| v.as_str()).unwrap_or("").to_uppercase()}).to_string()
        }),
        "crash" => {
            // 立即退出且不刷新 stdout——模拟插件进程崩溃
            std::process::exit(101);
        }
        "sleep_ms" => {
            let ms = req.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            serde_json::json!({"code": 0, "output": "{\"ok\":true,\"data\":\"woke\"}"})
        }
        other => serde_json::json!({
            "code": 2,
            "output": serde_json::json!({"ok": false, "code": 2, "err": format!("unknown op: {other}")}).to_string()
        }),
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    eprintln!("process-echo started"); // stderr 是自由诊断通道
    for line in stdin.lock().lines().map_while(Result::ok) {
        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // 非 JSON 行直接忽略
        };
        let id = msg.get("id").and_then(|v| v.as_u64());
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        match (method, id) {
            ("initialize", Some(id)) => respond(
                &mut stdout,
                id,
                serde_json::json!({
                    "id": "tool.echo", "name": "Echo",
                    "version": "0.2.0", "type": "tool"
                }),
            ),
            ("call", Some(id)) => {
                let input = msg
                    .pointer("/params/input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                respond(&mut stdout, id, handle_call(&input));
            }
            ("shutdown", _) => break,
            (_, Some(id)) => respond(
                &mut stdout,
                id,
                serde_json::json!({"code": -3, "output": "{\"ok\":false}"}),
            ),
            (_, None) => {}
        }
    }
}
