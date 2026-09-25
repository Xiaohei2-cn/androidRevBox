//! 特权执行层（D037 定案的候选②；D038 是其硬约束）。
//!
//! 为什么不是「Agent 直接以 root 跑」：实测把产物用 `su -c` 起成 root 后，adbd 连不上它的
//! 抽象 socket（`avc: denied { connectto } comm="adbd" tcontext=u:r:su:s0
//! tclass=unix_stream_socket`，见 §9 D037），要绕只能改 ROM 策略，与产品定位相反。
//! 所以 Agent 保持 shell 身份（socket 链路已全线验证），需要 root 的**动作**在这里起
//! `su` 子进程执行。
//!
//! D038 的落法：本模块**不接受调用方给的命令**，只接受「代码里写死的脚本模板 + 已校验的
//! 参数」。校验失败的参数直接拒绝，不进入字符串拼接；因此 su 不会变成绕过
//! 「系统包保护 / 允许根白名单」的通用逃生口。新增特权动作的姿势是：加一个模板 +
//! 加一条 typed 方法 + 加单测与真机腿，而不是开一个「传命令」的 API。

use std::time::Duration;

use agent_protocol::{AgentError, ErrorCode};
use tokio::process::Command;

/// 特权动作必须有界：KernelSU 授权弹窗没点时会一直挂着。
const SU_TIMEOUT: Duration = Duration::from_secs(20);
/// 允许出现的安装路径前缀：只可能是 APK 安装目录与托管暂存目录。
const ALLOWED_PREFIXES: &[&str] = &["/data/app/", "/data/local/tmp/", "/data/adb/modules/"];
/// 路径里禁止的字符：任何一个都可能破坏 `su -c '<脚本>'` 的单引号包裹或引入命令分隔。
/// 注意**不放** `~` 与 `=`：`/data/app/~~AbCd==/pkg-xYz==/` 是安装目录的真实形状，
/// 而这两个字符出现在路径中间不会被 shell 展开（`~` 只在词首有效）。
const FORBIDDEN_IN_PATH: &[char] = &[
    '\'', '"', '`', '$', ';', '&', '|', '<', '>', '(', ')', '{', '}', '*', '?', ' ', '\t', '\n',
    '\\', '!', '#', '[', ']', '^', '\0',
];

/// 特权脚本模板：把暂存件原子装到目标路径。
///
/// - 先 `cp` 到同目录临时名，再 `mv -f` 改名 —— 同分区 rename 是原子的，
///   任何一步失败都不会留下被截断的目标文件；
/// - 权限/属主用**原目标**的（替换时）；目标原本不存在（`extractNativeLibs=false`
///   的 App 在 `lib/arm64` 里根本没有文件）就退回**所在目录**的上下文 + `0644 root:root`，
///   这一组合已在 Pixel 6 / Android 14 实测能被应用加载（见 AR8.4 真机腿）；
/// - SELinux 上下文：`chcon --reference` 拿原件，拿不到就照目录；两者都失败只
///   `restorecon`，都不成也不改判定——文件内容对不对由 sha256 复核说了算；
/// - 结尾 `sync`：只 write 不 fsync，掉电后可能就是「目录项在、内容没落盘」。
pub(crate) fn install_script(staged: &str, target: &str, tmp: &str, dir: &str) -> String {
    format!(
        "set -e; MODE=$(stat -c %a {target} 2>/dev/null || echo 644); \
         OWNER=$(stat -c %U:%G {target} 2>/dev/null || echo root:root); \
         CONTEXT=$(stat -c %C {target} 2>/dev/null || stat -c %C {dir} 2>/dev/null || echo ''); \
         cp -f {staged} {tmp}; chmod $MODE {tmp}; chown $OWNER {tmp} 2>/dev/null || true; \
         if [ -n \"$CONTEXT\" ]; then chcon \"$CONTEXT\" {tmp} 2>/dev/null || restorecon {tmp} 2>/dev/null || true; fi; \
         mv -f {tmp} {target}; sync; echo INSTALLED"
    )
}

/// 回滚脚本：把备份装回目标；没有备份（原本不存在该文件）就把目标删掉。
///
/// 仍然走「同目录临时名 + rename」，回滚本身也必须原子——否则失败一次就留下半个备份文件。
pub(crate) fn rollback_script(backup: &str, target: &str, restore_backup: bool) -> String {
    if restore_backup {
        format!(
            "if [ -f {backup} ]; then MODE=$(stat -c %a {target} 2>/dev/null || echo 644); \
             CONTEXT=$(stat -c %C {target} 2>/dev/null || echo ''); \
             cp -f {backup} {target}.arttmp && chmod $MODE {target}.arttmp && \
             if [ -n \"$CONTEXT\" ]; then chcon \"$CONTEXT\" {target}.arttmp 2>/dev/null || true; fi; \
             sync && mv -f {target}.arttmp {target} && echo ROLLED_BACK; else echo NO_BACKUP; fi"
        )
    } else {
        format!("rm -f {target}; sync; echo REMOVED")
    }
}

/// 备份脚本（权限跟随原文件，便于 shell 侧读回做校验）。
pub(crate) fn backup_script(target: &str, backup: &str) -> String {
    format!("cp -f {target} {backup}; chmod 644 {backup}; sync; echo BACKED_UP")
}

/// 终止指定进程的特权脚本：**先核身份，再发信号**。
///
/// 为什么需要它：`process.kill` 的普通路径以 shell 发信号，杀不动 root 进程；而桌面侧原来的
/// 做法是 `su -c "kill -9 <pid>"` —— 只按一个数字杀。数字会复用：界面读到 pid 与用户点下
/// 「停止」之间哪怕隔几秒，那个 pid 也可能已经换了主人，于是"停掉 auth-server"变成
/// 随机杀掉某个无关进程。这条脚本把 AR6.3/AR7.3 已经定下的规矩搬到提权路径上：
/// 先比 `/proc/<pid>/comm`（同样截到 15 字符），对不上就一个信号都不发。
///
/// 参数：`pid` 必须是 >0 的数字，`comm` 只允许 `[A-Za-z0-9._-]`（长度 ≤15）；
/// 两者都由调用方校验后传入，脚本里不做任何字符串拼接式的"看起来安全"。
pub(crate) fn kill_verified_script(pid: u32, comm: &str) -> String {
    format!(
        "if [ ! -d /proc/{pid} ]; then echo KILL_GONE; echo {KILL_SENTINEL}; exit 0; fi; \
         C=$(cut -d \" \" -f2 /proc/{pid}/stat 2>/dev/null | tr -d \"()\"); \
         if [ \"$C\" != \"{comm}\" ]; then echo \"KILL_MISMATCH $C\"; echo {KILL_SENTINEL}; exit 0; fi; \
         kill {pid} 2>/dev/null; attempt=0; \
         while kill -0 {pid} 2>/dev/null && [ \"$attempt\" -lt 20 ]; do sleep 0.05; attempt=$((attempt + 1)); done; \
         if kill -0 {pid} 2>/dev/null; then kill -9 {pid} 2>/dev/null; fi; \
         attempt=0; while kill -0 {pid} 2>/dev/null && [ \"$attempt\" -lt 20 ]; do sleep 0.05; attempt=$((attempt + 1)); done; \
         if kill -0 {pid} 2>/dev/null; then echo STILL_ALIVE; else echo KILLED; fi; echo {KILL_SENTINEL}"
    )
}

/// `kill_verified_script` 的结束哨兵（三条出口都带它，见函数注释）。
pub(crate) const KILL_SENTINEL: &str = "ARTKILL_DONE";

/// 身份校验用的进程名白名单：只可能是 comm 的形状（截到 15 字符），
/// 任何 shell 元字符、空格、斜杠都在这里被挡掉。
pub(crate) fn validate_comm(comm: &str) -> Result<(), AgentError> {
    if comm.is_empty() || comm.len() > 15 {
        return Err(reject("comm_length", "进程名长度为 1..=15 字节"));
    }
    if !comm
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(reject(
            "comm_charset",
            format!("进程名含白名单外的字符: {comm:?}"),
        ));
    }
    Ok(())
}

/// 读 `/proc/<pid>/<file>` 详情的脚本模板（AR10.6 的按需读取）。
///
/// 参数**没有一个是路径**：`pid` 只能是数字、`file` 只能来自协议枚举、`max_lines` 会被
/// 夹紧成整数，路径是这里自己拼出来的。所以这条模板不构成"传任意路径就能提权读"的口子
/// （对比 `validate_path`：那里必须卡前缀与后缀，因为路径本身是外部给的）。
///
/// 输出形状（解析端 `parse_proc_read_output` 逐字对齐）：
/// ```text
/// ARTPROC_TOTAL
/// <总行数>
/// ARTPROC_GONE          # 进程已经不在了（提前结束，不会有 TOTAL）
/// ARTPROC_MISSING       # 进程在、但这个文件没有（如 32 位进程的 /proc/<pid>/map_files）
/// ARTPROC_BODY
/// <前 N 行>
/// ARTPROC_END
/// ```
/// 用 `awk END{print NR}` 而不是 `wc -l` 数行：`cmdline` 结尾没有换行，`wc -l` 会数成 0
/// （真机实测），而"0 行却有内容"会变成一个说不清的自相矛盾。
pub(crate) fn proc_read_script(pid: u32, file: &str, max_lines: u32) -> String {
    let path = format!("/proc/{pid}/{file}");
    // 三个分支都必须以哨兵收尾：`run_privileged` 是拿哨兵判断"脚本走完了"，
    // 早退时不带哨兵就会被当成"应答形状不对"，把一句本来说得清的"进程不在了"
    // 变成 Internal（真机第一次跑这条腿就是这样）。
    format!(
        "if [ ! -d /proc/{pid} ]; then echo ARTPROC_TOTAL; echo ARTPROC_GONE; echo ARTPROC_END; exit 0; fi; \
         if [ ! -e {path} ]; then echo ARTPROC_TOTAL; echo ARTPROC_MISSING; echo ARTPROC_END; exit 0; fi; \
         echo ARTPROC_TOTAL; awk 'END{{print NR}}' {path} 2>/dev/null || echo -1; \
         echo ARTPROC_BODY; sed -n '1,{max_lines}p' {path} 2>/dev/null; printf '\n'; \
         echo ARTPROC_END"
    )
}

/// 提权读 `/proc` 的哨兵：`run_privileged` 用它判断脚本走完了。
pub(crate) const PROC_READ_SENTINEL: &str = "ARTPROC_END";

/// 参数校验的公共部分：长度、前缀白名单、无 `..`、无 shell 元字符。
fn check_shape(path: &str) -> Result<(), AgentError> {
    if path.is_empty() || path.len() > 512 {
        return Err(reject("path_length", "路径长度不合法"));
    }
    if !ALLOWED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return Err(reject(
            "path_prefix_denied",
            format!("特权脚本只允许操作 {ALLOWED_PREFIXES:?} 下的路径: {path}"),
        ));
    }
    if path.contains("..") {
        return Err(reject("path_traversal", format!("路径不得包含 ..: {path}")));
    }
    if path.chars().any(|c| FORBIDDEN_IN_PATH.contains(&c)) {
        return Err(reject(
            "path_charset",
            format!("路径含会破坏 su 包裹的字符: {path}"),
        ));
    }
    Ok(())
}

/// 文件路径校验：在公共检查之上再加后缀白名单，避免把任意文件当成安装目标/备份件。
pub(crate) fn validate_path(path: &str) -> Result<(), AgentError> {
    check_shape(path)?;
    if !path.ends_with(".so") && !path.ends_with(".artbak") && !path.ends_with(".arttmp") {
        return Err(reject(
            "path_suffix",
            format!("只允许 .so/.artbak/.arttmp: {path}"),
        ));
    }
    Ok(())
}

/// 目录路径校验（只用于 `stat`/`chcon --reference` 的上下文参照物，脚本不写它）。
///
/// 目录名没有 `.so` 后缀，所以不能复用 `validate_path`；但它同样是拼进脚本的参数，
/// 必须过同一套前缀 + 字符集检查，并且不允许以 `/` 结尾（避免拼出歧义路径）。
pub(crate) fn validate_dir(path: &str) -> Result<(), AgentError> {
    check_shape(path)?;
    if path.ends_with('/') {
        return Err(reject(
            "path_dir_slash",
            format!("目录参数不应以 / 结尾: {path}"),
        ));
    }
    Ok(())
}

/// 托管二进制名白名单（AR9.1）：**只允许文件名**，路径由设备侧自己拼在托管目录里。
///
/// 不接受调用方传完整路径是有意的：那样等于让宿主指定「以 root 执行哪个文件」，
/// 特权层就变成通用执行器了。名字规则同 so 名但去掉 `.so` 约束（`frida-server` 没后缀）。
pub(crate) fn validate_binary_name(name: &str) -> Result<(), AgentError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.contains('/')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'));
    if ok {
        Ok(())
    } else {
        Err(reject(
            "invalid_binary_name",
            format!("二进制名只能是字母数字与 _.-+，不含路径分隔: {name}"),
        ))
    }
}

/// 监听地址与端口（AR9.1）。地址**只收两个确定值**，端口必须 ≥ 1024：
/// 低端口在 Android 上属特权组所有，root 起的服务也不该占；而多放行一个字符
/// 就是往脚本里塞未经校验的内容。
pub(crate) fn validate_listen(addr: &str, port: u16) -> Result<(), AgentError> {
    if addr != "127.0.0.1" && addr != "0.0.0.0" {
        return Err(reject(
            "invalid_bind_address",
            format!("绑定地址只允许 127.0.0.1 或 0.0.0.0: {addr}"),
        ));
    }
    if port < 1024 {
        return Err(reject(
            "privileged_port",
            format!("端口必须 ≥ 1024，收到 {port}"),
        ));
    }
    Ok(())
}

/// 以 root 后台拉起 frida-server（AR9.1）。
///
/// `nohup setsid ... &` 是实测形状：直接后台起的进程会随 su 会话结束一起没了，
/// setsid 脱离会话 + PPID 交给 init 才活得下来（Pixel 6 实测 `Uid 0 / PPID 1`）。
/// 脚本**不负责判断成功**—— pid、uid、LISTEN 全部由 Rust 侧复核，
/// 因为 `$!` 拿到的是 setsid 的 pid，与最终服务 pid 不保证相同。
pub(crate) fn frida_start_script(name: &str, addr: &str, port: u16, log: &str) -> String {
    format!(
        "cd /data/local/tmp; : > {log}; chmod 666 {log}; \
         test -x ./{name}; nohup setsid ./{name} -l {addr}:{port} > {log} 2>&1 & \
         sleep 1; echo FRIDA_STARTED"
    )
}

/// 以 root 停掉 frida-server（AR9.1）：**先核身份再杀**。
///
/// 逐个 `pidof` 结果都重新读一次 cmdline，避免 pid 复用杀错进程；
/// 读不到身份的（别人的进程挤在同一名字下）只报不杀。
pub(crate) fn frida_stop_script(name: &str) -> String {
    format!(
        "FOUND=0; DENIED=0; for P in $(pidof {name} 2>/dev/null); do FOUND=1; \
         A=$(tr '\\0' ' ' < /proc/$P/cmdline 2>/dev/null | cut -d' ' -f1); A=${{A##*/}}; \
         if [ \"$A\" = \"{name}\" ]; then kill $P 2>/dev/null || DENIED=1; else DENIED=1; fi; \
         done; if [ $FOUND = 0 ]; then echo FRIDA_NOT_RUNNING; \
         elif [ $DENIED = 1 ]; then echo FRIDA_KILL_DENIED; else echo FRIDA_STOPPED; fi"
    )
}

/// 参数被拒：`invalid_request` + `reason`。让 Desktop/测试能分清「形状不对」与
/// 「设备不让做」，而不是把所有失败糊成一句 internal。
fn reject(reason: &str, message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::InvalidRequest, message)
        .with_details(serde_json::json!({ "reason": reason }))
}

/// 执行固定脚本。stdout 必须是模板里约定的哨兵词，否则算失败。
pub(crate) async fn run_privileged(
    step: &str,
    script: &str,
    expect: &str,
) -> Result<String, AgentError> {
    let output = tokio::time::timeout(SU_TIMEOUT, Command::new("su").args(["-c", script]).output())
        .await
        .map_err(|_| {
            AgentError::new(
                ErrorCode::DeadlineExceeded,
                format!("{step} 超时（su 授权未应答或卡住）"),
            )
        })?
        .map_err(|error| {
            AgentError::new(
                ErrorCode::PermissionDenied,
                format!("{step} 无法执行 su: {error}"),
            )
            .with_details(serde_json::json!({ "reason": "su_unavailable" }))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(AgentError::new(
            ErrorCode::PermissionDenied,
            format!(
                "{step} 提权执行失败: {}",
                if stderr.is_empty() { &stdout } else { &stderr }
            ),
        )
        .with_details(serde_json::json!({ "reason": "su_failed", "exit": output.status.code() })));
    }
    if !stdout.lines().any(|line| line.trim() == expect) {
        return Err(AgentError::new(
            ErrorCode::Internal,
            format!("{step} 未回 expected 哨兵 {expect}，实际: {stdout}"),
        )
        .with_details(serde_json::json!({ "reason": "sentinel_missing" })));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_script_verifies_identity_before_signalling() {
        // 顺序很重要：先比 comm，再 kill —— 反过来说"停掉 auth-server"就可能杀到复用了
        // 同一个 pid 的无关进程
        let script = kill_verified_script(9727, "auth-server");
        let stat_at = script.find("/proc/9727/stat").expect("要读 stat");
        let kill_at = script.find("kill 9727").expect("要发信号");
        assert!(stat_at < kill_at, "身份核验必须在发信号之前: {script}");
        assert!(script.contains("KILL_MISMATCH"), "{script}");
        assert!(script.contains(KILL_SENTINEL));
        // 三条出口都要收尾：缺哨兵时 run_privileged 会把"名字对不上"报成 Internal
        assert_eq!(script.matches(KILL_SENTINEL).count(), 3, "{script}");
        // 参数不进脚本：能破坏 su 包裹或引入分隔符的名字直接拒
        assert!(validate_comm("auth-server").is_ok());
        for bad in [
            "",
            "a b",
            "$(id)",
            "x;rm -rf /",
            "a'b",
            "/sbin/x",
            "x|y",
            "名称",
        ] {
            assert!(validate_comm(bad).is_err(), "{bad} 不该被接受");
        }
        assert!(validate_comm("0123456789abcde").is_ok(), "15 字符是上限");
        assert!(
            validate_comm("0123456789abcdef").is_err(),
            "超过 15 字符要拒"
        );
    }

    #[test]
    fn privileged_paths_reject_anything_that_could_break_the_su_wrapper() {
        let ok = [
            "/data/app/~~a==/com.x-b==/lib/arm64/libfoo.so",
            "/data/local/tmp/app-reverse-tools-so/com.x-1/libfoo.so",
            "/data/local/tmp/app-reverse-tools-so/com.x-1/libfoo.so.artbak",
        ];
        for path in ok {
            assert!(validate_path(path).is_ok(), "{path} 应该合法");
        }
        let bad = [
            "",
            "/etc/hosts",
            "/data/app/../adb/x.so",
            "/data/local/tmp/lib.so; rm -rf /",
            "/data/local/tmp/$(id).so",
            "/data/local/tmp/`id`.so",
            "/data/local/tmp/a'b.so",
            "/data/local/tmp/a b.so",
            "/data/local/tmp/x.txt",
            "/data/data/com.x/libfoo.so",
        ];
        for path in bad {
            let error = validate_path(path).expect_err(&format!("{path:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{path:?}");
            assert!(error.details.is_some(), "{path:?} 必须带 reason");
        }
    }

    /// 目录参数走另一条校验：不要求 `.so` 后缀，但前缀/字符集/尾斜杠同样严格。
    #[test]
    fn dir_validator_allows_context_reference_dirs_only() {
        assert!(
            validate_dir("/data/app/~~a==/com.x-b==/lib/arm64").is_ok(),
            "真实 native lib 目录必须可作为上下文参照"
        );
        for bad in [
            "/data/local/tmp/app-reverse-tools-so/",
            "/data/data/com.x/lib/arm64",
            "/data/app/../../etc",
            "/data/app/$(id)",
        ] {
            let error = validate_dir(bad).expect_err(&format!("{bad:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad:?}");
        }
    }

    /// 脚本模板只能长这样：路径已校验过，且没有把任何调用方内容当成命令。
    #[test]
    fn install_script_is_a_fixed_template_with_validated_paths_only() {
        let script = install_script(
            "/data/local/tmp/app-reverse-tools-so/com.y-1/a.so",
            "/data/app/~~x/com.y-z/lib/arm64/a.so",
            "/data/app/~~x/com.y-z/lib/arm64/a.so.arttmp",
            "/data/app/~~x/com.y-z/lib/arm64",
        );
        assert!(script.starts_with("set -e;"));
        assert!(script.ends_with("echo INSTALLED"));
        // rename 必须来自同目录的临时文件，不能直接 cp 覆盖目标
        assert!(script.contains("mv -f /data/app/~~x/com.y-z/lib/arm64/a.so.arttmp"));
        assert!(script.contains("sync"));
        // 目标不存在时上下文退回所在目录（真机实测新增文件必须这样才加载得到）
        assert!(script.contains("stat -c %C /data/app/~~x/com.y-z/lib/arm64"));
        // 除了这四个已校验路径，脚本里不得出现别的绝对路径
        for token in script.split_whitespace() {
            if token.starts_with('/') {
                assert!(
                    [
                        "/data/local/tmp/app-reverse-tools-so/com.y-1/a.so",
                        "/data/app/~~x/com.y-z/lib/arm64/a.so",
                        "/data/app/~~x/com.y-z/lib/arm64/a.so.arttmp",
                        "/data/app/~~x/com.y-z/lib/arm64",
                        "/data/app/~~x/com.y-z/lib/arm64/a.so.arttmp",
                    ]
                    .iter()
                    .any(|p| token == *p || token.starts_with(p)),
                    "脚本里出现未校验路径: {token}"
                );
            }
        }
    }

    #[test]
    fn rollback_script_chooses_restore_or_remove_by_prior_existence() {
        let restore = rollback_script("/x/a.so.artbak", "/x/a.so", true);
        assert!(restore.contains("cp -f /x/a.so.artbak /x/a.so.arttmp"));
        assert!(restore.contains("mv -f /x/a.so.arttmp /x/a.so"));
        assert!(restore.contains("NO_BACKUP"));
        let remove = rollback_script("/x/a.so.artbak", "/x/a.so", false);
        assert_eq!(remove, "rm -f /x/a.so; sync; echo REMOVED");
    }

    #[tokio::test]
    async fn run_privileged_reports_missing_su_instead_of_hanging() {
        // 宿主（macOS）上没有 su：必须是立刻可分辨的 permission_denied，而不是超时挂住
        let error = match tokio::time::timeout(
            Duration::from_secs(6),
            run_privileged("probe", "echo NOPE", "SENTINEL"),
        )
        .await
        {
            Ok(result) => result.expect_err("宿主上 su 不存在，必须报错"),
            Err(_) => panic!("su 不可用时必须快速失败，不能把会话拖住"),
        };
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        let reason = error.details.unwrap()["reason"].clone();
        // 没有 su 二进制 → su_unavailable；有（如 macOS 的 /usr/bin/su）但跑不起来 → su_failed。
        // 两者都必须是「快速失败的 permission_denied」，不能挂住也不能报 internal。
        assert!(
            reason == "su_unavailable" || reason == "su_failed",
            "实际理由: {reason}"
        );
    }
}
