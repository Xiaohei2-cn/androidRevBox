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

/// attach 目标：包名（is_safe_pkg_name）或纯数字 pid。
pub fn validate_attach_target(target: &str) -> CoreResult<()> {
    if !target.is_empty() && target.chars().all(|c| c.is_ascii_digit()) {
        return Ok(());
    }
    if super::adb::is_safe_pkg_name(target) {
        return Ok(());
    }
    Err(CoreError::InvalidInput(format!(
        "目标要填包名或纯数字 pid（当前：{target}）"
    )))
}

/// frida 的注入目标（对应 CLI 的三种写法，分开表达、不靠"空字符串"表达意思）。
///
/// `Frontmost` 就是 `frida -UF` 里那个 `-F`：**附加设备当前前台应用，调用方不需要知道
/// 包名，也不用去挖 pid**。以前 attach 只有"必须给个目标"一种形状，于是留空点启动
/// 就换来一句伪装成程序故障的报错——那是设计错，不是用户错。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FridaTarget {
    /// 设备当前前台应用（frida `-F`）
    Frontmost,
    /// 指定进程：包名或纯数字 pid
    Attach(String),
    /// 冷启动注入指定包名（frida `-f`）
    Spawn(String),
}

/// 界面参数 → 注入目标。**attach 且目标留空 = 附加前台**（用户要的默认语义）。
///
/// 放在这一层而不是前端：前端有两条入口（行内「启动」与双击），少挡一条就还是会把空目标
/// 发下来；口径只留一处，才不会各写各的。
pub fn resolve_target(spawn: bool, target: &str) -> FridaTarget {
    let trimmed = target.trim();
    if spawn {
        return FridaTarget::Spawn(trimmed.to_string());
    }
    if trimmed.is_empty() {
        return FridaTarget::Frontmost;
    }
    FridaTarget::Attach(trimmed.to_string())
}

/// 组装 frida_runner.py 的参数数组（不含可执行文件）。
/// 纪律（§8-5）：结构化 args 直传、`--` 前缀长选项、脚本名已白名单校验；
/// 脚本内容永不进命令行，只传绝对路径。
pub fn build_runner_args(
    runner_path: &str,
    usb_serial: Option<&str>,
    remote_endpoint: Option<&str>,
    target: &FridaTarget,
    script_abs_path: &str,
) -> CoreResult<Vec<String>> {
    if usb_serial.is_some() && remote_endpoint.is_some() {
        return Err(CoreError::InvalidInput(
            "USB 与远程模式互斥，只能选其一".into(),
        ));
    }
    let mut args: Vec<String> = vec!["-u".to_string(), runner_path.to_string()];
    if let Some(serial) = usb_serial {
        if serial.trim().is_empty() {
            return Err(CoreError::InvalidInput("USB 模式缺少设备 serial".into()));
        }
        args.push("--usb".to_string());
        args.push(serial.trim().to_string());
    }
    if let Some(endpoint) = remote_endpoint {
        let (host, port) = parse_remote_endpoint(endpoint)?;
        args.push("--remote".to_string());
        args.push(format!("{host}:{port}"));
    }
    match target {
        // 不带任何目标参数：前台是谁由设备回答（见 frida_runner.py 的 --frontmost）
        FridaTarget::Frontmost => args.push("--frontmost".to_string()),
        FridaTarget::Attach(name) => {
            let name = name.trim();
            if name.is_empty() {
                // 走到这里说明调用方绕过了 resolve_target
                return Err(CoreError::InvalidInput(
                    "附加指定进程要填包名或 pid；想附加当前前台请把目标留空（等价 frida -F）"
                        .into(),
                ));
            }
            validate_attach_target(name)?;
            args.push("--attach".to_string());
            args.push(name.to_string());
        }
        FridaTarget::Spawn(pkg) => {
            let pkg = pkg.trim();
            // spawn 目标必须是包名：纯数字串会被误当 pid（is_safe_pkg_name 允许数字段）
            let is_pid = !pkg.is_empty() && pkg.chars().all(|c| c.is_ascii_digit());
            if pkg.is_empty() || is_pid || !super::adb::is_safe_pkg_name(pkg) {
                return Err(CoreError::InvalidInput(format!(
                    "Spawn 模式目标必须是包名（当前：{pkg}）"
                )));
            }
            args.push("--spawn".to_string());
            args.push(pkg.to_string());
        }
    }
    if script_abs_path.trim().is_empty() {
        return Err(CoreError::InvalidInput("脚本路径不能为空".into()));
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
    fn attach_with_no_target_means_frontmost_not_an_error() {
        // 用户口径：attach 就该自动包名（frida -UF 的 -F），留空不是错误
        assert_eq!(resolve_target(false, ""), FridaTarget::Frontmost);
        assert_eq!(resolve_target(false, "   "), FridaTarget::Frontmost);
        assert_eq!(
            resolve_target(false, "com.x.y"),
            FridaTarget::Attach("com.x.y".into())
        );
        assert_eq!(
            resolve_target(true, "com.x.y"),
            FridaTarget::Spawn("com.x.y".into())
        );
        // spawn 时留空仍然是错（没有包名没法冷启动），但报错得说人话
        match resolve_target(true, "") {
            FridaTarget::Spawn(pkg) => assert!(pkg.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn runner_args_shape_usb_spawn() {
        let args = build_runner_args(
            "/r/frida_runner.py",
            Some("emulator-5554"),
            None,
            &FridaTarget::Spawn("com.x.y".into()),
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
            &FridaTarget::Attach("1234".into()),
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
    fn runner_args_frontmost_carries_no_target_at_all() {
        // 关键形状：--frontmost 后面**不能**跟着一个目标串，否则 runner 的互斥组会打架
        let args = build_runner_args(
            "/r.py",
            Some("PIXEL-1"),
            None,
            &FridaTarget::Frontmost,
            "/w/01hook_tcp.js",
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "-u",
                "/r.py",
                "--usb",
                "PIXEL-1",
                "--frontmost",
                "--script",
                "/w/01hook_tcp.js",
            ]
        );
    }

    #[test]
    fn missing_input_is_not_an_internal_error() {
        // 这几条都是"用户还没填/填错"，历史上全部顶着「内部错误」出现（真机反馈过）
        let cases = vec![
            build_runner_args(
                "/r.py",
                Some("s"),
                Some("127.0.0.1:1"),
                &FridaTarget::Frontmost,
                "/w/a.js",
            ),
            build_runner_args("/r.py", Some(""), None, &FridaTarget::Frontmost, "/w/a.js"),
            build_runner_args(
                "/r.py",
                Some("s"),
                None,
                &FridaTarget::Spawn("1234".into()),
                "/w/a.js",
            ),
            build_runner_args(
                "/r.py",
                Some("s"),
                None,
                &FridaTarget::Spawn("".into()),
                "/w/a.js",
            ),
            build_runner_args(
                "/r.py",
                Some("s"),
                None,
                &FridaTarget::Attach("a b".into()),
                "/w/a.js",
            ),
            build_runner_args("/r.py", Some("s"), None, &FridaTarget::Frontmost, ""),
            // 端点写错也属同一类：输入问题不是程序故障
            parse_remote_endpoint("127.0.0.1").map(|_| Vec::<String>::new()),
        ];
        for case in cases {
            let error = case.expect_err("非法输入必须被拒");
            assert_eq!(
                error.code(),
                "INVALID_INPUT",
                "输入问题不能伪装成程序故障: {error}"
            );
            assert!(!error.to_string().contains("内部错误"), "{error}");
        }
    }

    #[test]
    fn empty_attach_string_is_refused_instead_of_silently_becoming_frontmost() {
        // Attach("") 只能来自绕过 resolve_target 的调用方：这里明确拒，
        // 免得"附加前台"这种事在链路中间被悄悄推断出来。
        let error = build_runner_args(
            "/r.py",
            Some("s"),
            None,
            &FridaTarget::Attach("  ".into()),
            "/w/a.js",
        )
        .expect_err("空目标不该被当成任意一种 attach");
        assert_eq!(error.code(), "INVALID_INPUT");
    }
}
