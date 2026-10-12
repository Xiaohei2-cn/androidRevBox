//! Frida 工作台适配层（P10）：宿主侧纯逻辑——脚本文件名校验、远端端点解析、
//! 通道握手探活脚本。全部无 IO，可无设备单测（设计文档 §6.4、§8-2）。
//!
//! 这里**不再**有任何 runner 参数组装：结构化终端与 `frida_runner.py` 已整体退役
//! （起会话统一走真终端里的官方 `frida_tools.repl`），残下的只有官方 REPL 也用得到
//! 的三件：文件名校验、端点解析、握手探活。

use crate::core::error::{CoreError, CoreResult};

/// 校验工作目录下的脚本文件名：与托管二进制同源白名单
/// （仅字母数字与 `_ . -`、不以 `.` 或 `-` 开头——后者防被当作命令行参数），
/// 且必须以 `.js` 结尾。
pub fn is_safe_js_script_name(name: &str) -> bool {
    if !super::adb::is_safe_hosted_name(name) {
        return false;
    }
    !name.starts_with('-') && name.ends_with(".js") && name.len() > ".js".len()
}

/// 远程端点 `host:port` 解析：host 仅允许字母数字、`.`、`-`（IPv4/localhost），
/// port 1–65535。不拼 shell，但保持「含糊输入进不来」的同一纪律。
pub fn parse_remote_endpoint(spec: &str) -> CoreResult<(String, u16)> {
    let spec = spec.trim();
    let Some((host, port)) = spec.rsplit_once(':') else {
        return Err(CoreError::InvalidInput(format!(
            "远程端点需为 host:port 形式（当前：{spec}）"
        )));
    };
    let host = host.trim();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(CoreError::InvalidInput(format!(
            "远程端点 host 非法: {host}"
        )));
    }
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| CoreError::InvalidInput(format!("远程端点端口非法: {spec}")))?;
    if port == 0 {
        return Err(CoreError::InvalidInput("远程端点端口不能为 0".into()));
    }
    Ok((host.to_string(), port))
}

/// 通道握手探活脚本：用**工具配置的那个解释器**连设备上的 frida-server，问一件最小的事。
///
/// 为什么问"能不能枚举进程"而不是 connect 一下 TCP：端口活着 ≠ 能用。版本不匹配时
/// frida 直接回 `unable to communicate with remote frida-server`，而 TCP 探活照样是绿的
/// —— 真机上就出现过"三项全绿、会话起不来"。argv 只收模式与目标，不拼字符串。
pub const CHANNEL_PROBE_SCRIPT: &str = r#"
import sys
mode = sys.argv[1]
target = sys.argv[2]
try:
    import frida
except Exception as e:
    print("ERR", type(e).__name__, str(e)[:200])
    raise SystemExit(0)
try:
    if mode == "remote":
        dev = frida.get_device_manager().add_remote_device(target)
        where = "远程 " + target
    else:
        dev = frida.get_device(target, timeout=8)
        where = "USB " + target
    n = len(dev.enumerate_processes())
    print("OK", n, where)
except Exception as e:
    print("ERR", type(e).__name__, " ".join(str(e).split())[:200])
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_name_whitelist() {
        assert!(is_safe_js_script_name("hook-ssl.js"));
        assert!(is_safe_js_script_name("a_b.1.js"));
        // 非 js / 目录名 / 隐藏文件 / 参数注入形态 / 空
        assert!(!is_safe_js_script_name("run.sh"));
        assert!(!is_safe_js_script_name(".hidden.js"));
        assert!(!is_safe_js_script_name("-x.js"));
        assert!(!is_safe_js_script_name("with space.js"));
        assert!(!is_safe_js_script_name("a/b.js"));
        assert!(!is_safe_js_script_name("a;rm -rf.js"));
        assert!(!is_safe_js_script_name(".js"));
        assert!(!is_safe_js_script_name(""));
    }

    #[test]
    fn remote_endpoint_parses() {
        assert_eq!(
            parse_remote_endpoint("127.0.0.1:27042").unwrap(),
            ("127.0.0.1".into(), 27042)
        );
        assert_eq!(
            parse_remote_endpoint(" localhost : 27043 ").unwrap(),
            ("localhost".into(), 27043)
        );
        assert!(parse_remote_endpoint("127.0.0.1").is_err());
        assert!(parse_remote_endpoint("127.0.0.1:0").is_err());
        assert!(parse_remote_endpoint("127.0.0.1:70000").is_err());
        assert!(parse_remote_endpoint("bad host:27042").is_err());
        assert!(parse_remote_endpoint(":27042").is_err());
        assert!(parse_remote_endpoint("127.0.0.1:27042;id").is_err());
    }
}
