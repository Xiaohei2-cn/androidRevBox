//! ADB 适配器：纯逻辑（命令构造 + 输出解析），不执行进程。
//! 拆出来是为了无真机也能在 CI 单测（总案 §6 / PHASES §6.3 的 MockAdapter 思路）。
//! 一切命令以 executable + args 数组形式给出，禁止拼 shell 字符串。
//!
//! 本模块的构造器/解析器按协议一次性冻结，业务接线分期进行
//! （forward/reverse/reboot/cat_preview/join 等随 P7 系统面板与文件面板接入），
//! 因此允许暂时无 lib 级调用者。
#![allow(dead_code)]

use serde::Serialize;

/// adb 可执行文件路径解析失败时的语义错误（调用方转 CoreError）。
#[derive(Debug, PartialEq, Eq)]
pub enum AdbError {
    /// 未配置且 PATH 中找不到 adb
    NotFound,
}

/// `adb devices -l` 的一行设备信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub serial: String,
    pub state: String,
    pub transport: String,
    pub model: String,
}

impl DeviceEntry {
    /// 处于可用状态（可执行 shell/安装等）
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }
}

/// 从 adb 路径候选中选出第一个存在的；全不存在返回 NotFound。
/// candidates 由调用方（Service）提供：DB 配置 → ANDROID_HOME → 常见 SDK 路径 → "adb"。
pub fn resolve_adb_path<F>(candidates: &[String], exists: F) -> Result<String, AdbError>
where
    F: Fn(&str) -> bool,
{
    candidates
        .iter()
        .find(|p| is_usable_adb(p, &exists))
        .cloned()
        .ok_or(AdbError::NotFound)
}

/// 裸名 "adb"（交给 PATH 解析）始终视为可用；带路径的需存在。
fn is_usable_adb<F>(path: &str, exists: F) -> bool
where
    F: Fn(&str) -> bool,
{
    if path == "adb" {
        return true;
    }
    exists(path)
}

/// 构造 adb 全局参数前缀：指定 serial 时插入 `-s <serial>`。
pub fn build_args<S: AsRef<str>>(serial: Option<&str>, subcommand: &[S]) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(s) = serial {
        args.push("-s".to_string());
        args.push(s.to_string());
    }
    args.extend(subcommand.iter().map(|s| s.as_ref().to_string()));
    args
}

/// 解析 `adb devices -l` 输出。
pub fn parse_devices(stdout: &str) -> Vec<DeviceEntry> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("List of devices")
            || trimmed.starts_with("*")
            || trimmed.starts_with("adb server")
            || trimmed.starts_with("daemon")
        {
            continue;
        }
        // serial 与 state 之间是空白对齐；后续 key:value 属性
        let mut it = trimmed.split_whitespace();
        let Some(serial) = it.next() else { continue };
        let Some(state) = it.next() else { continue };
        let mut model = String::new();
        let mut has_usb = false;
        for tok in it {
            if let Some(v) = tok.strip_prefix("model:") {
                model = v.to_string();
            } else if tok == "usb" || tok.starts_with("usb:") {
                // 新版 adb -l 输出 "usb:1-2"（带端口），旧版是裸 "usb"
                has_usb = true;
            }
        }
        let transport = classify_transport(serial, has_usb);
        out.push(DeviceEntry {
            serial: serial.to_string(),
            state: state.to_string(),
            transport,
            model,
        });
    }
    out
}

fn classify_transport(serial: &str, has_usb: bool) -> String {
    if serial.starts_with("emulator-") {
        "emulator".to_string()
    } else if serial.contains(':') {
        // host:port → 无线/网络 adb
        "network".to_string()
    } else if has_usb {
        "usb".to_string()
    } else {
        "unknown".to_string()
    }
}

/// 解析 `adb shell getprop` 输出为 map（`key: [value]` 每行一条）。
pub fn parse_getprop(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        // 形如 `[ro.product.model]: [Pixel 7]`
        let Some(colon) = line.find("]: [") else {
            continue;
        };
        let key = line[1..colon].trim().to_string(); // 去掉前导 [
        let value = line[colon + 4..].trim_end_matches(']').trim().to_string();
        map.insert(key, value);
    }
    map
}

/// 设备信息 DTO（由 getprop 抽取常用字段）
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub model: String,
    pub manufacturer: String,
    pub android_version: String,
    pub sdk_int: String,
    pub serial: String,
    /// wlan0 IPv4（读取失败为 None，如未连 Wi-Fi / 纯 USB 无网）
    pub ip: Option<String>,
}

/// 从已解析的 getprop map 组装设备信息。
pub fn device_info_from_props(
    serial: &str,
    props: &std::collections::HashMap<String, String>,
) -> DeviceInfo {
    let g = |k: &str| props.get(k).cloned().unwrap_or_default();
    DeviceInfo {
        model: g("ro.product.model"),
        manufacturer: g("ro.product.manufacturer"),
        android_version: g("ro.build.version.release"),
        sdk_int: g("ro.build.version.sdk"),
        serial: serial.to_string(),
        ip: None,
    }
}

/// `adb shell ls -l` 的一行（toybox 格式，宽松解析）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub symlink: Option<String>,
}

/// 解析 `ls -l` 行。示例：
/// `drwxrwx--x 2 root root 3452 2024-01-01 08:00 storage`
/// `-rw-rw---- 1 u0_a1 u0_a1 1024 2024-01-01 08:00 a.txt`
/// `lrwxrwxrwx 1 root root 11 ... init -> /init`
pub fn parse_ls_long(line: &str) -> Option<FileEntry> {
    let line = line.trim_end_matches('\r').trim();
    if line.is_empty() {
        return None;
    }
    // 目录汇总行 "total 12" 跳过
    if line.starts_with("total ") {
        return None;
    }
    let mut it = line.split_whitespace();
    let perms = it.next()?;
    // 权限字段首字符表示类型：d 目录、l 链接、- 常规、其余按非目录
    let is_dir = perms.starts_with('d');
    let is_link = perms.starts_with('l');
    // 跳过 links/owner/group/size 中的前三个，size 是数值列
    let mut size = 0i64;
    let mut seen_size = false;
    let mut rest_cols: Vec<&str> = Vec::new();
    // 结构：links owner group size date time name...
    for (i, col) in it.enumerate() {
        match i {
            0..=2 => {} // links, owner, group
            3 => {
                size = col.parse().unwrap_or(0);
                seen_size = true;
            }
            _ => rest_cols.push(col), // date, time, name...
        }
    }
    if !seen_size {
        return None;
    }
    // rest_cols = [date, time, name...]（date/time 共 2 列）
    if rest_cols.len() < 3 {
        return None;
    }
    let name_and_target = rest_cols[2..].join(" ");
    let (name, symlink) = if is_link {
        match name_and_target.split_once(" -> ") {
            Some((n, t)) => (n.to_string(), Some(t.to_string())),
            None => (name_and_target, None),
        }
    } else {
        (name_and_target, None)
    };
    if name.is_empty() {
        return None;
    }
    Some(FileEntry {
        name,
        is_dir,
        size,
        symlink,
    })
}

/// 按 `/` 分段剥离 `..` 与空段（供 join_remote_path 复用）。
fn clean_path_segments(p: &str) -> Vec<&str> {
    p.split('/')
        .filter(|s| *s != ".." && !s.is_empty())
        .collect()
}

/// 安全拼接设备侧路径：分段剥离 `..`（基础防穿越，真实边界仍由设备端约束）。
pub fn join_remote_path(dir: &str, name: &str) -> String {
    let mut segs = clean_path_segments(dir);
    segs.extend(clean_path_segments(name));
    format!("/{}", segs.join("/"))
}

/// adb 版本信息（`adb version` 解析结果）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbVersionInfo {
    /// "Android Debug Bridge version X" 里的 X
    pub version: String,
    /// "Version Y" 行（platform-tools 构建号），无则空
    pub build: String,
}

/// 解析 `adb version` 输出；识别不出版本号返回 None。
pub fn parse_version(stdout: &str) -> Option<AdbVersionInfo> {
    let mut version = None;
    let mut build = String::new();
    for line in stdout.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("Android Debug Bridge version ") {
            version = Some(v.to_string());
        } else if let Some(b) = t.strip_prefix("Version ") {
            build = b.to_string();
        }
    }
    version.map(|v| AdbVersionInfo { version: v, build })
}

/// 按优先级构造 adb 可执行文件候选路径：
/// ANDROID_HOME/platform-tools → ANDROID_SDK_ROOT/platform-tools → PATH 逐目录。
/// exe_names 传 ["adb"] 或 Windows 的 ["adb.exe","adb.bat","adb.cmd"]。
pub fn build_adb_candidates(
    exe_names: &[&str],
    android_home: Option<&str>,
    android_sdk_root: Option<&str>,
    path_env: Option<&str>,
    sep: char,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for root in [android_home, android_sdk_root].into_iter().flatten() {
        let root = root.trim_end_matches(['/', '\\']);
        for exe in exe_names {
            out.push(format!("{root}{sep}platform-tools{sep}{exe}"));
        }
    }
    if let Some(path) = path_env {
        for dir in path.split(if sep == '\\' { ';' } else { ':' }) {
            if dir.is_empty() {
                continue;
            }
            let dir = dir.trim_end_matches(['/', '\\']);
            for exe in exe_names {
                out.push(format!("{dir}{sep}{exe}"));
            }
        }
    }
    out.dedup();
    out
}

/// 当前平台的 adb 可执行文件名集合（Windows 附带批处理包装）。
pub fn adb_exe_names() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["adb.exe", "adb.bat", "adb.cmd"]
    } else {
        vec!["adb"]
    }
}

// ===== 子命令构造器（全部返回 args 数组，由 build_args 前置 -s <serial>）=====

pub fn cmd_version() -> Vec<String> {
    vec!["version".into()]
}
pub fn cmd_devices() -> Vec<String> {
    vec!["devices".into(), "-l".into()]
}
pub fn cmd_getprop() -> Vec<String> {
    vec!["shell".into(), "getprop".into()]
}
pub fn cmd_shell(command: &str) -> Vec<String> {
    vec!["shell".into(), command.into()]
}
pub fn cmd_list_packages(third_party_only: bool) -> Vec<String> {
    let mut v = vec![
        "shell".into(),
        "pm".into(),
        "list".into(),
        "packages".into(),
    ];
    if third_party_only {
        v.push("-3".into());
    }
    v
}
pub fn cmd_force_stop(pkg: &str) -> Vec<String> {
    vec!["shell".into(), "am".into(), "force-stop".into(), pkg.into()]
}
pub fn cmd_launch(pkg: &str) -> Vec<String> {
    vec![
        "shell".into(),
        "monkey".into(),
        "-p".into(),
        pkg.into(),
        "-c".into(),
        "android.intent.category.LAUNCHER".into(),
        "1".into(),
    ]
}
pub fn cmd_uninstall(pkg: &str) -> Vec<String> {
    vec!["uninstall".into(), pkg.into()]
}
pub fn cmd_install(apk: &str) -> Vec<String> {
    vec!["install".into(), "-r".into(), apk.into()]
}
pub fn cmd_push(local: &str, remote: &str) -> Vec<String> {
    vec!["push".into(), local.into(), remote.into()]
}
pub fn cmd_pull(remote: &str, local: &str) -> Vec<String> {
    vec!["pull".into(), remote.into(), local.into()]
}
pub fn cmd_logcat(filter: Option<&str>) -> Vec<String> {
    let mut v = vec!["logcat".into()];
    if let Some(f) = filter.filter(|f| !f.trim().is_empty()) {
        v.extend(["-e".into(), f.into()]);
    }
    v
}
pub fn cmd_ls(path: &str) -> Vec<String> {
    vec!["shell".into(), format!("ls -lA {path}")]
}
pub fn cmd_cat_preview(path: &str, max_bytes: u64) -> Vec<String> {
    vec!["shell".into(), format!("head -c {max_bytes} {path}")]
}
/// 读取 wlan0 的 IP（用户指定命令）：`adb -s <serial> shell ip addr show wlan0`
pub fn cmd_ip_addr() -> Vec<String> {
    vec!["shell".into(), "ip addr show wlan0".into()]
}
/// 端口转发：`adb -s <serial> forward <local> <remote>`（local/remote 形如 tcp:8080）
#[allow(dead_code)]
pub fn cmd_forward(local: &str, remote: &str) -> Vec<String> {
    vec!["forward".into(), local.into(), remote.into()]
}
/// 列出该设备的全部转发规则：`adb -s <serial> forward --list`
pub fn cmd_forward_list() -> Vec<String> {
    vec!["forward".into(), "--list".into()]
}
/// 删除一条转发：`adb -s <serial> forward --remove <local>`；local 为 None 时 --remove-all
pub fn cmd_forward_remove(local: Option<&str>) -> Vec<String> {
    match local {
        Some(l) => vec!["forward".into(), "--remove".into(), l.into()],
        None => vec!["forward".into(), "--remove-all".into()],
    }
}
/// 端口反向转发（设备侧服务暴露给宿主）：`adb -s <serial> reverse <remote> <local>`
pub fn cmd_reverse(remote: &str, local: &str) -> Vec<String> {
    vec!["reverse".into(), remote.into(), local.into()]
}
pub fn cmd_reboot() -> Vec<String> {
    vec!["reboot".into()]
}

/// 解析 `pm list packages` 输出行（`package:xxx`）。
pub fn parse_packages(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter_map(|l| l.strip_prefix("package:").map(String::from))
        .collect()
}

/// 解析 `ip addr show wlan0` 输出中的 wlan0 IPv4 地址（inet 192.168.1.5/24）。
/// 多块网卡/多地址时取第一个；解析不到返回 None（如设备用 eth0 或未连 Wi-Fi）。
pub fn parse_wlan0_ip(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let t = line.trim_end_matches('\r').trim();
        // 形如 "inet 192.168.1.5/24 brd 192.168.1.255 scope global wlan0"
        if let Some(rest) = t.strip_prefix("inet ") {
            let token = rest.split_whitespace().next().unwrap_or("");
            if let Some(ip) = token.split('/').next() {
                if ip.split('.').count() == 4 && ip.parse::<std::net::Ipv4Addr>().is_ok() {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

/// 校验 forward 规格字符串：仅允许 `tcp:<1-65535>` 与 `localabstract:<name>`、
/// `localreserved:<name>`（host 侧只接受这三种常用形态；防参数注入额外 adb 参数）。
pub fn is_valid_forward_spec(spec: &str) -> bool {
    let Some((scheme, value)) = spec.split_once(':') else {
        return false;
    };
    match scheme {
        "tcp" => value
            .parse::<u16>()
            .map(|p| p > 0)
            .unwrap_or(false)
            .then_some(())
            .is_some(),
        "localabstract" | "localreserved" => {
            !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        }
        _ => false,
    }
}

/// 规格归一：纯数字（用户习惯只填端口）自动按 `tcp:` 处理，其余原样返回。
/// 归一后再走 is_valid_forward_spec 校验，两侧输入都更宽容。
pub fn normalize_forward_spec(spec: &str) -> String {
    let s = spec.trim();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        format!("tcp:{s}")
    } else {
        s.to_string()
    }
}

/// 解析 `forward --list` 输出行：`<serial> <local> <remote>`（空格分隔）。
pub fn parse_forward_list(stdout: &str) -> Vec<(String, String, String)> {
    stdout
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.to_string(),
                it.next()?.to_string(),
                it.next()?.to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_first_existing() {
        let cands = vec![
            "/no/such/adb".to_string(),
            "/opt/adb".to_string(),
            "adb".to_string(),
        ];
        let exists = |p: &str| p == "/opt/adb";
        assert_eq!(resolve_adb_path(&cands, exists).unwrap(), "/opt/adb");
    }

    #[test]
    fn resolve_falls_back_to_bare_adb() {
        let cands = vec!["/no/such/adb".to_string(), "adb".to_string()];
        let exists = |_p: &str| false;
        assert_eq!(resolve_adb_path(&cands, exists).unwrap(), "adb");
    }

    #[test]
    fn resolve_not_found_when_no_candidate_usable() {
        let cands = vec!["/no/such/adb".to_string()];
        let exists = |_p: &str| false;
        assert_eq!(resolve_adb_path(&cands, exists), Err(AdbError::NotFound));
    }

    #[test]
    fn build_args_with_and_without_serial() {
        assert_eq!(
            build_args(Some("emulator-5554"), &["shell", "getprop"]),
            vec!["-s", "emulator-5554", "shell", "getprop"]
        );
        assert_eq!(build_args(None, &["devices", "-l"]), vec!["devices", "-l"]);
    }

    #[test]
    fn parse_devices_typical_output() {
        let out = "List of devices attached\nemulator-5554          device product:sdk model:Pixel_7 device:emu64a transport_id:1\n0123456789abcdef       unauthorized usb 1-2 transport_id:2\n";
        let devs = parse_devices(out);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].serial, "emulator-5554");
        assert!(devs[0].is_ready());
        assert_eq!(devs[0].transport, "emulator");
        assert_eq!(devs[0].model, "Pixel_7");
        assert!(!devs[1].is_ready());
        assert_eq!(devs[1].state, "unauthorized");
        assert_eq!(devs[1].transport, "usb");
    }

    #[test]
    fn parse_devices_empty_and_daemon_lines() {
        let out = "* daemon started successfully\nList of devices attached\n\n";
        assert!(parse_devices(out).is_empty());
    }

    #[test]
    fn parse_devices_network_transport() {
        let devs = parse_devices("192.168.1.5:5555   device model:X transport_id:9\n");
        assert_eq!(devs[0].transport, "network");
    }

    #[test]
    fn parse_getprop_extracts_kv() {
        let out =
            "[ro.product.model]: [Pixel 7]\n[ro.build.version.release]: [14]\n[garbage line]\n";
        let map = parse_getprop(out);
        assert_eq!(
            map.get("ro.product.model").map(String::as_str),
            Some("Pixel 7")
        );
        assert_eq!(
            map.get("ro.build.version.release").map(String::as_str),
            Some("14")
        );
        assert!(!map.contains_key("garbage line"));
    }

    #[test]
    fn device_info_from_props_maps_fields() {
        let mut map = std::collections::HashMap::new();
        map.insert("ro.product.model".to_string(), "Pixel 7".to_string());
        map.insert("ro.product.manufacturer".to_string(), "Google".to_string());
        map.insert("ro.build.version.release".to_string(), "14".to_string());
        map.insert("ro.build.version.sdk".to_string(), "34".to_string());
        let info = device_info_from_props("ser1", &map);
        assert_eq!(info.model, "Pixel 7");
        assert_eq!(info.manufacturer, "Google");
        assert_eq!(info.android_version, "14");
        assert_eq!(info.sdk_int, "34");
        assert_eq!(info.serial, "ser1");
    }

    #[test]
    fn parse_ls_long_dir_file_link() {
        let dir = parse_ls_long("drwxrwx--x 2 root root 3452 2024-01-01 08:00 storage").unwrap();
        assert_eq!(dir.name, "storage");
        assert!(dir.is_dir);

        let file = parse_ls_long("-rw-rw---- 1 u0_a1 u0_a1 1024 2024-01-01 08:00 a.txt").unwrap();
        assert_eq!(file.name, "a.txt");
        assert_eq!(file.size, 1024);
        assert!(!file.is_dir);

        let link =
            parse_ls_long("lrwxrwxrwx 1 root root 11 2024-01-01 08:00 init -> /init").unwrap();
        assert_eq!(link.name, "init");
        assert_eq!(link.symlink.as_deref(), Some("/init"));
    }

    #[test]
    fn parse_ls_long_handles_name_with_spaces_and_skips_total() {
        let e = parse_ls_long("-rw-r--r-- 1 root root 5 2024-01-01 08:00 my file.txt").unwrap();
        assert_eq!(e.name, "my file.txt");
        assert!(parse_ls_long("total 12").is_none());
        assert!(parse_ls_long("").is_none());
    }

    #[test]
    fn join_remote_path_basic() {
        assert_eq!(join_remote_path("/sdcard", "a.txt"), "/sdcard/a.txt");
        assert_eq!(join_remote_path("/sdcard/", "a.txt"), "/sdcard/a.txt");
        assert_eq!(join_remote_path("/", "etc"), "/etc");
        assert_eq!(join_remote_path("", "etc"), "/etc");
        // 防穿越：.. 被剥离，不会跳出目录
        assert_eq!(join_remote_path("/sdcard", "../secret"), "/sdcard/secret");
        assert_eq!(join_remote_path("/sdcard", "a/../../b"), "/sdcard/a/b");
        assert_eq!(
            join_remote_path("/sdcard/../../etc", "passwd"),
            "/sdcard/etc/passwd"
        );
    }
    #[test]
    fn parse_version_typical_and_missing() {
        let out =
            "Android Debug Bridge version 1.0.41\nVersion 37.0.0-14910828\nInstalled as /opt/adb\n";
        let v = parse_version(out).unwrap();
        assert_eq!(v.version, "1.0.41");
        assert_eq!(v.build, "37.0.0-14910828");
        assert!(parse_version("garbage").is_none());
    }

    #[test]
    fn parse_devices_new_format_usb_marker() {
        // 新版 adb devices -l：USB 标记是 "usb:1-2"（带冒号），非裸 "usb"
        let out = "List of devices attached\nABC123       device usb:1-2 product:foo model:Pixel transport_id:1\n";
        let devs = parse_devices(out);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].transport, "usb", "usb:1-2 应识别为 USB 设备");
    }

    #[test]
    fn build_adb_candidates_priority_and_path_scan() {
        let c = build_adb_candidates(
            &["adb"],
            Some("/home/u/Android/sdk/"),
            None,
            Some("/usr/bin:/opt/adb-dir:"),
            '/',
        );
        assert_eq!(c[0], "/home/u/Android/sdk/platform-tools/adb");
        assert!(c.contains(&"/usr/bin/adb".to_string()));
        assert!(c.contains(&"/opt/adb-dir/adb".to_string()));
        assert!(!c.iter().any(|p| p.ends_with("::adb")));
        assert!(
            !c.iter().any(|p| p.contains("//adb")),
            "不应产生重复斜杠: {c:?}"
        );
    }

    #[test]
    fn build_adb_candidates_windows_separators() {
        let c = build_adb_candidates(
            &["adb.exe"],
            Some("C:\\Android\\sdk"),
            None,
            Some("C:\\platform-tools;C:\\x"),
            '\\',
        );
        assert_eq!(c[0], "C:\\Android\\sdk\\platform-tools\\adb.exe");
        assert!(c.contains(&"C:\\platform-tools\\adb.exe".to_string()));
    }

    #[test]
    fn subcommand_builders() {
        assert_eq!(cmd_logcat(Some("crash")), ["logcat", "-e", "crash"]);
        assert_eq!(cmd_logcat(None), ["logcat"]);
        assert_eq!(cmd_logcat(Some("  ")), ["logcat"]);
        assert_eq!(cmd_install("/tmp/a.apk"), ["install", "-r", "/tmp/a.apk"]);
        assert_eq!(cmd_push("/a", "/b/c"), ["push", "/a", "/b/c"]);
        assert_eq!(cmd_list_packages(true).last().unwrap(), "-3");
        assert_eq!(cmd_launch("com.x")[3], "com.x");
        assert_eq!(
            cmd_force_stop("com.x"),
            ["shell", "am", "force-stop", "com.x"]
        );
        assert_eq!(cmd_cat_preview("/f", 512), ["shell", "head -c 512 /f"]);
        // 带 serial 前缀组合
        assert_eq!(
            build_args(Some("s1"), &cmd_devices()),
            ["-s", "s1", "devices", "-l"]
        );
    }

    #[test]
    fn parse_packages_strips_prefix() {
        let out = "package:com.android.settings\r\npackage:com.demo\r\n";
        assert_eq!(
            parse_packages(out),
            vec!["com.android.settings", "com.demo"]
        );
    }

    #[test]
    fn parse_wlan0_ip_extracts_inet() {
        let out = "24: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\r\n\
                   inet 192.168.1.5/24 brd 192.168.1.255 scope global wlan0\r\n\
                   valid_lft forever preferred_lft forever\r\n";
        assert_eq!(parse_wlan0_ip(out).as_deref(), Some("192.168.1.5"));
        assert!(parse_wlan0_ip("no match").is_none());
        // IPv6 inet 行不误报
        let v6 = "inet6 fe80::1/64 scope link\n";
        assert_eq!(parse_wlan0_ip(v6), None);
    }

    #[test]
    fn forward_spec_validation() {
        assert!(is_valid_forward_spec("tcp:8080"));
        assert!(is_valid_forward_spec("tcp:1"));
        assert!(is_valid_forward_spec("localabstract:foo.bar_baz-x"));
        assert!(!is_valid_forward_spec("tcp:0"));
        assert!(!is_valid_forward_spec("tcp:99999"));
        assert!(!is_valid_forward_spec("tcp:8080 extra"));
        assert!(!is_valid_forward_spec("shell:rm"));
        assert!(!is_valid_forward_spec("8080"));
    }

    #[test]
    fn forward_spec_normalize_bare_port() {
        // 用户只填端口 → 自动 tcp:；带前缀 / 非数字原样
        assert_eq!(normalize_forward_spec("22222"), "tcp:22222");
        assert_eq!(normalize_forward_spec(" 8080 "), "tcp:8080");
        assert_eq!(normalize_forward_spec("tcp:8080"), "tcp:8080");
        assert_eq!(
            normalize_forward_spec("localabstract:foo"),
            "localabstract:foo"
        );
        // 归一后必须过校验（回归：裸端口曾被拒）
        assert!(is_valid_forward_spec(&normalize_forward_spec("22222")));
        assert!(!is_valid_forward_spec(&normalize_forward_spec("99999")));
    }

    #[test]
    fn parse_forward_list_rows() {
        let out = "ABC123 tcp:8080 tcp:9000\r\nABC123 tcp:5555 localabstract:foo\r\n";
        let rows = parse_forward_list(out);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("ABC123".into(), "tcp:8080".into(), "tcp:9000".into()));
        assert_eq!(rows[1].2, "localabstract:foo");
        assert!(parse_forward_list("").is_empty());
    }

    #[test]
    fn forward_command_builders() {
        assert_eq!(cmd_ip_addr(), ["shell", "ip addr show wlan0"]);
        assert_eq!(
            cmd_forward("tcp:8080", "tcp:9000"),
            ["forward", "tcp:8080", "tcp:9000"]
        );
        assert_eq!(cmd_forward_list(), ["forward", "--list"]);
        assert_eq!(cmd_forward_remove(Some("tcp:1")), ["forward", "--remove", "tcp:1"]);
        assert_eq!(cmd_forward_remove(None), ["forward", "--remove-all"]);
        // -s 绑定组合（多设备隔离的关键）
        assert_eq!(
            build_args(Some("s1"), &cmd_forward_list()),
            ["-s", "s1", "forward", "--list"]
        );
    }
}
