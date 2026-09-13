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
    /// 原始权限串（如 "-rwxr-xr-x"）；文件浏览 UI 可忽略
    #[serde(default)]
    pub perms: String,
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
        perms: perms.to_string(),
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

// ===== 二进制托管（/data/local/tmp）=====

/// 托管目录固定路径（用户指定默认）。
pub const HOSTED_DIR: &str = "/data/local/tmp";

/// 一个被托管的二进制（tmp 目录下的 ELF 文件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedBinary {
    pub name: String,
    pub path: String,
    pub size: i64,
    /// 原始权限串（如 "-rwxr-xr-x"）
    pub perms: String,
    /// owner 有执行位 → 绿色；否则红色（可 chmod 赋予）
    pub has_exec: bool,
}

/// 校验托管文件名：仅允许安全字符（防 shell 注入 / 路径逃逸）。
/// 拒绝空、以 . 开头（隐藏文件）、含 `/`、空格或 shell 元字符。
pub fn is_safe_hosted_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// ls 权限串（如 "-rwxr-xr-x"）是否 owner 可执行（第 4 字符 x/s）。
pub fn perms_has_exec(perms: &str) -> bool {
    // -rwxr-xr-x → 索引 3 是 owner 的 execute
    perms.chars().nth(3).is_some_and(|c| c == 'x' || c == 's')
}

/// `file /data/local/tmp/x` 输出是否识别为 ELF（"ELF 64-bit ... executable"）。
pub fn is_elf_file_output(output: &str) -> bool {
    // 设备端 file 缺失时输出 "file: not found" 等，一律不算 ELF
    output.contains("ELF")
}

/// 组合 ls -l 与 file 输出为托管二进制列表：
/// - `ls_entries`：`ls -l <dir>` 已解析出的普通文件；
/// - `file_lines`：`file <dir>/*` 的输出行（每行 `<path>: <type>`）。
///
/// 只保留被 file 判定为 ELF 的文件。
pub fn hosted_binaries(
    ls_stdout: &str,
    file_stdout: &str,
) -> Vec<HostedBinary> {
    // 收集 ELF 命中的绝对路径集合
    let elf_paths: std::collections::HashSet<String> = file_stdout
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| is_elf_file_output(l))
        .filter_map(|l| l.split_once(':').map(|(p, _)| p.trim().to_string()))
        .collect();
    let mut out: Vec<HostedBinary> = Vec::new();
    for line in ls_stdout.lines() {
        let Some(fe) = parse_ls_long(line) else { continue };
        if fe.is_dir {
            continue;
        }
        let path = format!("{HOSTED_DIR}/{}", fe.name);
        if !elf_paths.contains(&path) {
            continue;
        }
        out.push(HostedBinary {
            name: fe.name.clone(),
            path,
            size: fe.size,
            has_exec: perms_has_exec(&fe.perms),
            perms: fe.perms,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 解析 `nohup ./x & echo $!` 的输出为首个整数 pid。
pub fn parse_run_pid(stdout: &str) -> Option<u32> {
    stdout
        .trim()
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
        .and_then(|l| l.parse::<u32>().ok())
}

/// 托管启动日志尾部 → 单行诊断（最后 3 个非空行，` | ` 连接，300 字符截断）。
/// 空日志返回 None（此时上层给出通用提示）。
pub fn run_log_diagnostics(log_tail: &str) -> Option<String> {
    let lines: Vec<&str> = log_tail
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let mut s = lines[lines.len().saturating_sub(3)..].join(" | ");
    if s.chars().count() > 300 {
        let cut = s
            .char_indices()
            .nth(300)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        s = s[..cut].to_string() + "…";
    }
    Some(s)
}

/// 托管运行日志路径（`<dir>/.<name>.run.log`，点开头：`file dir/*` 的
/// shell 通配不匹配隐藏文件，不会污染 ELF 列表）。
pub fn hosted_run_log(name: &str) -> String {
    format!("{HOSTED_DIR}/.{name}.run.log")
}

/// 以 root 执行：`su -c '<cmd>'`（Magisk/APatch/KSU 通用形式）。
/// 单引号包裹是必须的：adb shell 传输会把字符串交给设备端外层 shell
/// 二次分词，不引起来 `su -c cd /x && nohup y &` 的 `&&`/`&`/重定向会被
/// 外层解释，su 实际只收到 `cd`。调用方命令模板均不含单引号
/// （文件名过 is_safe_hosted_name 白名单，路径/日志名固定），包裹安全。
pub fn su_wrap(cmd: &str) -> String {
    debug_assert!(!cmd.contains('\''), "su_wrap 命令含单引号会破坏包裹");
    format!("su -c '{cmd}'")
}

/// 判定 `su -c id` 探测输出是否代表拿到了 root。
pub fn is_root_probe_ok(stdout: &str) -> bool {
    stdout.contains("uid=0")
}

/// 托管后台启动命令模板：`cd <dir>; nohup ./<name> >log 2>&1 & echo $!`。
/// ⚠️ 用 `;` 而非 `&&`：`&&` 优先级低于 `&`，会把整个 `cd && nohup`
/// 复合式后台化，`$!` 拿到子 shell pid 而非二进制 pid。
/// root=true 时整段 su -c 单引号包裹（外层 shell 不动 &/$!/重定向）。
pub fn hosted_run_cmd(name: &str, log: &str, root: bool) -> String {
    debug_assert!(is_safe_hosted_name(name));
    let inner = format!("cd {HOSTED_DIR}; nohup ./{name} >{log} 2>&1 & echo $!");
    if root {
        su_wrap(&inner)
    } else {
        inner
    }
}

/// 托管进程监听端口信息（/proc/net/tcp|tcp6 行解析结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenPort {
    /// 可读 IP（v4 点分；v6 压缩记法；0.0.0.0/:: 表示全网卡）
    pub address: String,
    pub port: u16,
    /// 0A = LISTEN
    pub listen: bool,
    pub family: &'static str, // "tcp" | "tcp6"
}

/// 小端字节序 hex → IPv4 点分（"0100007F" → 127.0.0.1）。
fn hex_ipv4(hex: &str) -> Option<String> {
    if hex.len() != 8 {
        return None;
    }
    let raw = u32::from_str_radix(hex, 16).ok()?;
    // /proc/net 以主机序（LE）打印 in_addr：to_le_bytes 原序即网络字节序
    let b = raw.to_le_bytes();
    Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
}

/// 大端字节序 hex → IPv6 压缩记法（32 hex chars，按 /proc 的 4×u32 小端组）。
fn hex_ipv6(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }
    // 每 8 hex = 一个 u32（小端存法同 v4），4 组拼出 16 字节
    let mut bytes = [0u8; 16];
    for (gi, group) in hex.as_bytes().chunks(8).enumerate() {
        let g = std::str::from_utf8(group).ok()?;
        let raw = u32::from_str_radix(g, 16).ok()?;
        let b = raw.to_le_bytes();
        bytes[gi * 4..gi * 4 + 4].copy_from_slice(&b);
    }
    let addr = std::net::Ipv6Addr::from(bytes);
    Some(addr.to_string())
}

/// 解析单行 `/proc/net/tcp` 或 `tcp6` 记录（可带 grep 输出的 `tcp:`/`tcp6:`
/// 行首前缀——先剥前缀再分词，否则行头粘成 `tcp:sl` 无法解析）：
/// `sl local_address rem_address st ...`。要求字段严格为 8/32 hex + ':' +
/// 4 hex 形态；遇假 hex（如 IPv6 组里的 "::"）跳过继续扫，不整行放弃。
fn parse_proc_net_line(line: &str, family: &'static str) -> Option<ListenPort> {
    let stripped = match family {
        "tcp6" => line.strip_prefix("tcp6:").or_else(|| line.strip_prefix("tcp:"))?,
        _ => line.strip_prefix("tcp:")?,
    };
    let toks: Vec<&str> = stripped.split_whitespace().collect();
    for (idx, tok) in toks.iter().enumerate() {
        let Some((ip_hex, port_hex)) = tok.split_once(':') else {
            continue;
        };
        let v6 = ip_hex.len() == 32;
        if (ip_hex.len() != 8 && !v6) || port_hex.len() != 4 {
            continue;
        }
        if !ip_hex.chars().all(|c| c.is_ascii_hexdigit())
            || !port_hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            continue;
        }
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Some(address) = (if v6 { hex_ipv6(ip_hex) } else { hex_ipv4(ip_hex) }) else {
            continue;
        };
        let state = toks.get(idx + 2).copied().unwrap_or("");
        return Some(ListenPort {
            address,
            port,
            listen: state.eq_ignore_ascii_case("0A"),
            family,
        });
    }
    None
}

/// 解析设备端 socket inode 匹配输出（每行形如
/// `tcp:   0      0 0100007F:1F90 00000000:0000 0A ...` 或 `tcp6: ...`，
/// 也兼容 grep 直接输出无 sl 前缀变体）。去重 + 只保留 LISTEN，端口升序。
pub fn parse_listening_ports(stdout: &str) -> Vec<ListenPort> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let fam = if line.trim_start().starts_with("tcp6:") {
            "tcp6"
        } else if line.trim_start().starts_with("tcp:") {
            "tcp"
        } else {
            continue;
        };
        let Some(p) = parse_proc_net_line(line, fam) else {
            continue;
        };
        if !p.listen {
            continue;
        }
        if seen.insert((p.port, p.address.clone(), p.family)) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.port.cmp(&b.port).then(a.family.cmp(b.family)).then(a.address.cmp(&b.address)));
    out
}

/// 托管二进制监听端口查询命令（用户指定）：
/// `for i in $(ls -l /proc/<pid>/fd 2>/dev/null | sed -n "s/.*socket:\[\(.*\)\].*/\1/p"); do grep "$i" /proc/net/tcp /proc/net/tcp6; done`
pub fn hosted_ports_cmd(pid: u32) -> String {
    format!(
        "for i in $(ls -l /proc/{pid}/fd 2>/dev/null | sed -n \"s/.*socket:\\[\\(.*\\)\\].*/\\1/p\"); do grep \"$i\" /proc/net/tcp /proc/net/tcp6; done"
    )
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
    fn hosted_name_safety() {
        assert!(is_safe_hosted_name("test"));
        assert!(is_safe_hosted_name("frida-server_16.5.9"));
        assert!(is_safe_hosted_name("lib-dump.so"));
        assert!(!is_safe_hosted_name(""));
        assert!(!is_safe_hosted_name(".hidden"));
        assert!(!is_safe_hosted_name("a/../b"));
        assert!(!is_safe_hosted_name("a b"));
        assert!(!is_safe_hosted_name("x; rm -rf /"));
        assert!(!is_safe_hosted_name("x`id`"));
        assert!(!is_safe_hosted_name("$IFS"));
    }

    #[test]
    fn perms_exec_detection() {
        assert!(perms_has_exec("-rwxr-xr-x"));
        assert!(perms_has_exec("-rwsr--r--"));
        // 目录位也看 owner x（目录过滤由调用方 is_dir 负责）
        assert!(perms_has_exec("drwxrwxrwx"));
        assert!(!perms_has_exec("-rw-r--r--"));
        assert!(!perms_has_exec("drw-r--r--"));
        assert!(!perms_has_exec(""));
    }

    #[test]
    fn hosted_binaries_filters_elf_only() {
        let ls = "-rwxr-xr-x 1 shell shell 10240 2026-01-01 08:00 test\r\n\
                  -rw-r--r-- 1 shell shell 2048 2026-01-01 08:00 noexec\r\n\
                  -rw-r--r-- 1 shell shell 100 2026-01-01 08:00 readme.txt\r\n\
                  drwxr-xr-x 2 shell shell 4096 2026-01-01 08:00 subdir\r\n";
        let file = "/data/local/tmp/test: ELF 64-bit LSB executable, ARM aarch64\r\n\
                    /data/local/tmp/noexec: ELF 64-bit LSB shared object, ARM aarch64\r\n\
                    /data/local/tmp/readme.txt: ASCII text\r\n\
                    /data/local/tmp/subdir: directory\r\n";
        let bins = hosted_binaries(ls, file);
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].name, "noexec");
        assert!(!bins[0].has_exec, "rw-r-- 无执行位");
        assert_eq!(bins[1].name, "test");
        assert!(bins[1].has_exec);
        assert_eq!(bins[1].path, "/data/local/tmp/test");
        // file 缺失/权限拒绝输出不误报
        assert!(hosted_binaries(ls, "file: not found").is_empty());
    }

    #[test]
    fn parse_run_pid_takes_first_number() {
        assert_eq!(parse_run_pid("12345\n"), Some(12345));
        assert_eq!(parse_run_pid("nohup: redirecting stderr\n99\n"), Some(99));
        assert_eq!(parse_run_pid("not a pid"), None);
        assert_eq!(parse_run_pid(""), None);
    }

    #[test]
    fn run_log_diagnostics_extracts_cause() {
        // 缺依赖库的典型 stderr（真机实况形态）
        let out = "CANNOT LINK EXECUTABLE \"./xhmfd1656-n\": library \"liblog.so\" not found: needed by main executable\n";
        let d = run_log_diagnostics(out).expect("有日志应给出死因");
        assert!(d.contains("CANNOT LINK EXECUTABLE"), "{d}");
        // 多行取最后三行、' | ' 连接
        let d = run_log_diagnostics("l1\nl2\nl3\nl4\n").unwrap();
        assert_eq!(d, "l2 | l3 | l4");
        // 空日志（进程静默自退）
        assert!(run_log_diagnostics("  \n \n").is_none());
        // 300 字符截断
        let long = "x".repeat(500);
        let d = run_log_diagnostics(&long).unwrap();
        assert!(d.chars().count() <= 301, "{}", d.chars().count());
        assert!(d.ends_with('…'));
    }

    #[test]
    fn hosted_run_log_hidden_in_dir() {
        assert_eq!(hosted_run_log("test"), "/data/local/tmp/.test.run.log");
    }

    #[test]
    fn su_wrap_quotes_whole_command() {
        // 关键：& $! > 2>&1 等必须整体进单引号，交给 root 内层 shell 解释
        let w = su_wrap("cd /data/local/tmp; nohup ./x >l 2>&1 & echo $!");
        assert!(w.starts_with("su -c 'cd /data/local/tmp;"));
        assert!(w.ends_with("echo $!'"));
        assert_eq!(w.matches('\'').count(), 2);
    }

    #[test]
    fn su_probe_output_recognized() {
        assert!(is_root_probe_ok("uid=0(root) gid=0(root) groups=0 root"));
        assert!(is_root_probe_ok("uid=0"));
        assert!(!is_root_probe_ok("su: uid=2000(shell)"));
        assert!(!is_root_probe_ok("su: inaccessible or not found"));
    }

    #[test]
    fn hosted_run_cmd_uses_semicolon_not_andand() {
        // 回归：&& 优先级低于 &，$! 会拿到子 shell 的 pid 而非二进制的
        let log = hosted_run_log("xhmfd1656-n");
        let cmd = hosted_run_cmd("xhmfd1656-n", &log, false);
        assert!(cmd.starts_with("cd /data/local/tmp; nohup ./xhmfd1656-n >"));
        assert!(cmd.ends_with("& echo $!"));
        assert!(!cmd.contains("&& nohup"), "禁用 && 连接：{cmd}");
        // root：整段单引号包裹（内层 & $! > 归 root shell 解释）
        let root_cmd = hosted_run_cmd("xhmfd1656-n", &log, true);
        assert!(root_cmd.starts_with("su -c 'cd /data/local/tmp;"));
        assert!(root_cmd.ends_with("echo $!'"));
        assert_eq!(root_cmd.matches('\'').count(), 2);
    }

    #[test]
    fn listening_ports_parses_v4_v6_and_hex() {
        // 真机典型形态：grep 多文件带 tcp:/tcp6: 前缀，1F90=8080，27042=0x69A2
        let out = "\
tcp:   1: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 10093 0 0000000000000000 1 0
tcp:   2: 00000000:69A2 00000000:0000 0A 00000000:00000000 00:00000000 00000000 10093 0 0000000000000000 1 0
tcp6:  3: 00000000000000000000000001000000:1F91 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 10093 0 0000000000000000 1 0
";
        let ports = parse_listening_ports(out);
        assert_eq!(ports.len(), 3);
        // 端口升序：8080(127.0.0.1 tcp) < 8081(::1 tcp6) < 27042(0.0.0.0 tcp)
        assert_eq!((ports[0].port, ports[0].address.as_str()), (8080, "127.0.0.1"));
        assert_eq!((ports[1].port, ports[1].address.as_str()), (8081, "::1"));
        assert_eq!(ports[1].family, "tcp6");
        assert_eq!((ports[2].port, ports[2].address.as_str()), (27042, "0.0.0.0"));
        assert!(ports.iter().all(|p| p.listen));
    }

    #[test]
    fn listening_ports_skips_non_listen_and_junk() {
        let out = "\
tcp:   1: 0100007F:1F90 0100007F:C000 01 00000000:00000000 00:00000000 00000000 10093 0 0000000000000000 1 0
sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
tcp: garbage line
";
        // 01 = ESTABLISHED 不计入；表头与杂行跳过
        assert!(parse_listening_ports(out).is_empty());
        assert!(parse_listening_ports("").is_empty());
    }

    #[test]
    fn listening_ports_dedup_across_v4_v6_mirrors() {
        // v4-mapped 双栈常见重复行
        let line = "tcp:   1: 00000000:69A2 00000000:0000 0A 0 0 0\n";
        let out = format!("{line}{line}");
        assert_eq!(parse_listening_ports(&out).len(), 1);
    }

    #[test]
    fn hosted_ports_cmd_shape() {
        let c = hosted_ports_cmd(30743);
        assert!(c.starts_with("for i in $(ls -l /proc/30743/fd 2>/dev/null"));
        assert!(c.ends_with("/proc/net/tcp /proc/net/tcp6; done"));
        // su -c 单引号包裹安全：内部无单引号
        assert!(!c.contains('\''));
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
