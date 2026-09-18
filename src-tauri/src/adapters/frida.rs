//! Frida 工作台适配层（P10）：宿主侧纯逻辑——脚本文件名校验、远端端点解析、
//! runner 参数组装。全部无 IO，可无设备单测（设计文档 §6.4、§8-2）。
//!
//! NDJSON 行协议（§5.2）的解析放在前端（回放能力白送，见 §6.3），
//! 宿主侧只定义协议常量的文档镜像，供 runner 与前端对齐。

use crate::core::error::{CoreError, CoreResult};

/// runner 行协议的事件类型（docs/frida-console-design.md §5.2）：
/// ready | log | send | error | exit。前端按 `t` 字段分派。
/// 宿主不解析行（协议解析在前端，回放能力白送，§6.3），常量仅作跨语言协议镜像。
#[allow(dead_code)]
pub mod protocol {
    pub const T_READY: &str = "ready";
    pub const T_LOG: &str = "log";
    pub const T_SEND: &str = "send";
    pub const T_ERROR: &str = "error";
    pub const T_EXIT: &str = "exit";
}

/// frida-server 默认端口（远程模式引导：端口转发建 tcp:27042 → tcp:27042）。
/// 前端默认值与之一致；宿主侧仅文档镜像。
#[allow(dead_code)]
pub const DEFAULT_FRIDA_PORT: u16 = 27_042;

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
        return Err(CoreError::Internal(format!(
            "远程端点需为 host:port 形式: {spec}"
        )));
    };
    let host = host.trim();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(CoreError::Internal(format!("远程端点 host 非法: {host}")));
    }
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| CoreError::Internal(format!("远程端点端口非法: {spec}")))?;
    if port == 0 {
        return Err(CoreError::Internal("远程端点端口不能为 0".into()));
    }
    Ok((host.to_string(), port))
}

/// attach 目标：包名（is_safe_pkg_name）或纯数字 pid。
pub fn validate_attach_target(target: &str) -> CoreResult<()> {
    if !target.is_empty() && target.chars().all(|c| c.is_ascii_digit()) {
        return Ok(());
    }
    if super::adb::is_safe_pkg_name(target) {
        return Ok(());
    }
    Err(CoreError::Internal(format!(
        "目标需为包名或纯数字 pid: {target}"
    )))
}

/// 组装 frida_runner.py 的参数数组（不含可执行文件）。
/// 纪律（§8-5）：结构化 args 直传、`--` 前缀长选项、脚本名已白名单校验；
/// 脚本内容永不进命令行，只传绝对路径。
pub fn build_runner_args(
    runner_path: &str,
    usb_serial: Option<&str>,
    remote_endpoint: Option<&str>,
    spawn: bool,
    target: &str,
    script_abs_path: &str,
) -> CoreResult<Vec<String>> {
    if usb_serial.is_some() && remote_endpoint.is_some() {
        return Err(CoreError::Internal("USB 与远程模式互斥，只能选其一".into()));
    }
    let mut args: Vec<String> = vec!["-u".to_string(), runner_path.to_string()];
    if let Some(serial) = usb_serial {
        if serial.trim().is_empty() {
            return Err(CoreError::Internal("USB 模式缺少设备 serial".into()));
        }
        args.push("--usb".to_string());
        args.push(serial.trim().to_string());
    }
    if let Some(endpoint) = remote_endpoint {
        let (host, port) = parse_remote_endpoint(endpoint)?;
        args.push("--remote".to_string());
        args.push(format!("{host}:{port}"));
    }
    if target.trim().is_empty() {
        return Err(CoreError::Internal("目标应用（包名或 pid）不能为空".into()));
    }
    if spawn {
        // spawn 目标必须是包名：纯数字串会被误当 pid（is_safe_pkg_name 允许数字段）
        let is_pid = !target.trim().is_empty() && target.trim().chars().all(|c| c.is_ascii_digit());
        if is_pid || !super::adb::is_safe_pkg_name(target.trim()) {
            return Err(CoreError::Internal(format!(
                "Spawn 模式目标必须是包名: {target}"
            )));
        }
        args.push("--spawn".to_string());
    } else {
        validate_attach_target(target.trim())?;
        args.push("--attach".to_string());
    }
    args.push(target.trim().to_string());
    if script_abs_path.trim().is_empty() {
        return Err(CoreError::Internal("脚本路径不能为空".into()));
    }
    args.push("--script".to_string());
    args.push(script_abs_path.to_string());
    Ok(args)
}

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

    #[test]
    fn attach_target_accepts_pkg_and_pid() {
        assert!(validate_attach_target("12345").is_ok());
        assert!(validate_attach_target("com.example.app").is_ok());
        assert!(validate_attach_target("com.example.app;id").is_err());
        assert!(validate_attach_target("").is_err());
    }

    #[test]
    fn runner_args_shape_usb_spawn() {
        let args = build_runner_args(
            "/r/frida_runner.py",
            Some("emulator-5554"),
            None,
            true,
            "com.x.y",
            "/w/hook.js",
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "-u",
                "/r/frida_runner.py",
                "--usb",
                "emulator-5554",
                "--spawn",
                "com.x.y",
                "--script",
                "/w/hook.js",
            ]
        );
    }

    #[test]
    fn runner_args_shape_remote_attach() {
        let args = build_runner_args(
            "/r/frida_runner.py",
            None,
            Some("127.0.0.1:27042"),
            false,
            "1234",
            "/w/hook.js",
        )
        .unwrap();
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--remote" && w[1] == "127.0.0.1:27042")
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--attach" && w[1] == "1234")
        );
    }

    #[test]
    fn runner_args_rejects_bad_combos() {
        // USB 与远程互斥
        assert!(
            build_runner_args(
                "/r.py",
                Some("s"),
                Some("127.0.0.1:1"),
                true,
                "com.x",
                "/w/a.js"
            )
            .is_err()
        );
        // spawn 必须包名
        assert!(
            build_runner_args("/r.py", Some("s"), None, true, "1234", "/w/a.js").is_err(),
            "pid 不能 spawn"
        );
        // 目标/脚本为空
        assert!(build_runner_args("/r.py", Some("s"), None, true, "", "/w/a.js").is_err());
        assert!(build_runner_args("/r.py", Some("s"), None, true, "com.x", "").is_err());
    }
}
