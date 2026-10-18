//! AR9.4 审计护栏：每个 `adb shell` / 长任务调用点必须**登记并归类**。
//!
//! 阶段文档 §AR9.4 要求「逐条分类为 Bootstrap、Transport、Raw Tool 或 Legacy
//! fallback，任何未登记的业务 shell 都必须迁移或写明临时删除阶段」。这句话如果
//! 只写在文档里，就等于「谁有空谁去看一眼」——AR7.4 做过一次分类，之后 AR8/AR9
//! 新增了 8 个调用点，没人复查看不出漂移。所以这里把它变成测试：
//! ① 新调用点不在表里 → 红；② 表里的函数被删/改名 → 红（防止登记与实现分叉）；
//! ③ 各类数量与文档记录不符 → 红；④ Legacy 条目必须写删除阶段。
//!
//! 扫描刻意跳过 `#[cfg(test)]` 模块与定义行：测试腿为了对照必然要拼 shell，
//! 那不是业务能力，而是回归证据（`raw_tool_boundary.rs` 同口径）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Category {
    /// Agent 自己的装机/会话生命周期：Agent 还没起来时只能用 adb，不可迁移
    Bootstrap,
    /// 文件传输与其收尾：adb 的本职，不伪装成业务语义
    Transport,
    /// 用户在 Shell/logcat 页主动敲的原始命令
    RawTool,
    /// 只读回退腿：Agent 侧已实现，旧链路保留对照，AR12.1 按阶段删除
    LegacyFallback,
    /// `root=true` 支路：Agent 以 shell 身份运行做不到（D026），待候选③
    RootBranch,
}

/// `(文件, 函数, 类别, 删除阶段)`。删除阶段 `None` 表示设计上保留、不进删除计划。
const REGISTRY: &[(&str, &str, Category, Option<&str>)] = &[
    (
        "src/adapters/agent_bootstrap.rs",
        "device_abi",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "install_artifact",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "rollback_install",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "start",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "stop",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "remove_remote_session",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "remote_sha256",
        Category::Bootstrap,
        None,
    ),
    (
        // AR12.5 自动连接的只读探测：查"设备上有没有在跑的 Agent"。它不可迁移
        // （Agent 没起来时没有别的通道可问），也不该被算成 Legacy 回退——它不产出
        // 任何业务答案，只决定要不要动手，所以类别是 Bootstrap。
        "src/adapters/agent_bootstrap.rs",
        "is_running",
        Category::Bootstrap,
        None,
    ),
    (
        "src/adapters/agent_bootstrap.rs",
        "remove_remote_file",
        Category::Bootstrap,
        None,
    ),
    (
        // 版本对不上时清掉设备侧副本，让下一次 `install_artifact` 真的推一遍。
        // 归 Bootstrap 而不是 Legacy 回退：它不属于任何业务能力，只在"握手说版本不对"
        // 这一条自愈路径上删一个文件；Agent 没连上时根本没有别的通道能做这件事。
        "src/adapters/agent_bootstrap.rs",
        "invalidate_remote_artifact",
        Category::Bootstrap,
        None,
    ),
    // Transport：push/pull/install 与 SO 替换的暂存件收尾
    (
        "src/services/device_service.rs",
        "start_push",
        Category::Transport,
        None,
    ),
    (
        "src/services/device_service.rs",
        "start_pull",
        Category::Transport,
        None,
    ),
    (
        "src/services/device_service.rs",
        "start_install",
        Category::Transport,
        None,
    ),
    (
        // adb_task 是"起一个 adb 长任务"的分发器本体，它自己也是一处真实的长任务调用点：
        // 把它登记上，而不是把它从扫描规则里绕过去（AR8.5 拆出 with_cleanup 版本时，
        // 这个点曾从审计里静默消失过一次）。
        "src/services/device_service.rs",
        "adb_task",
        Category::Transport,
        None,
    ),
    (
        "src/services/device_service.rs",
        "replace_native_library",
        Category::Transport,
        None,
    ),
    // 工具市场的一键安装（P13）：push 之后的三条 shell —— `chmod 0755`、读设备 ABI、
    // 安装后复核（`stat` + `test -x`）。三条都是幂等或只读的，归 Transport 而不是
    // Legacy fallback：它们不是"Agent 能力的旧回退腿"，而是 adb 的本职——把本机下好的
    // 文件送进 /data/local/tmp 并让它能跑。没走 TaskService 长任务，是因为安装是一条
    // **带校验的链**（下载 → 摘要 → 推送 → chmod → 回读），中途失败要说清断在哪一步，
    // 而不是丢一个任务 id 让人自己去日志里找。
    (
        "src/services/tool_market.rs",
        "install",
        Category::Transport,
        None,
    ),
    (
        "src/services/tool_market.rs",
        "device_abi",
        Category::Transport,
        None,
    ),
    (
        "src/services/tool_market.rs",
        "verify",
        Category::Transport,
        None,
    ),
    // Raw Tool：AR9.3 已限定只能由命令层为「用户主动发起的会话」调用。
    // （原来这里的 `raw_shell_task` 已随设备页 Shell 子标签换成真终端而整条删除：
    //   交互式 shell 需要 PTY，一条一条 `adb shell <cmd>` 拼不出来 —— 少一个调用点
    //   在这里留下事实，与当年 AR7.6 少一个点要改数是同一条纪律。）
    (
        "src/services/device_service.rs",
        "raw_logcat_task",
        Category::RawTool,
        None,
    ),
    // Legacy 只读回退腿
    (
        "src/services/device_service.rs",
        "hosted_binaries_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR7.2"),
    ),
    (
        "src/services/device_service.rs",
        "su_available_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR9.1"),
    ),
    // 原来这里登记的是 device_service::hosted_kill_legacy —— 一条
    // `adb shell su -c "kill -9 <pid>"` 的盲杀（只按数字、不核身份）。root 终止已改走
    // Agent 的固定提权脚本（先比 comm 再发信号），那个调用点整个被消灭，所以条目一起删：
    // 审计要求"登记与实现不能分叉"，少一个调用点也要在这里留一句话，说明它是被删掉的、
    // 不是漏登记的。AR12 的候选清单因此少一项。
    (
        "src/services/device_service.rs",
        "hosted_ports_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR7.2"),
    ),
    (
        "src/services/device_service.rs",
        "pids_by_port_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR6.2"),
    ),
    // 第七十一轮：`pkg_lib_dir_legacy` 连同它的回退腿一起删掉了（AR12.1 取证已过）。
    // 条目一起删是纪律要求 —— 登记与实现不能分叉；少一个调用点也必须在这里看得见。
    (
        "src/services/env_service.rs",
        "shell",
        Category::LegacyFallback,
        Some("AR12.1 after AR6.1"),
    ),
    // root=true 支路：做不到 typed 化，等候选③（模块 root companion）
    // 设备页文件写/读侧的 root 腿：Agent 以 shell 身份运行，root 属主的东西它改不动也读不到
    (
        // 设备页文件**读**侧的 root 腿（su -c）：`/data` 这类目录 shell 连 opendir 都进不去，
        // Agent 只能如实回"读不到"，界面上就成了"这个目录是空的"。读不受写侧两棵树的限制
        // （要看得到东西才谈得上逆向），但同样不许拿"读不到"冒充"空"。
        "src/services/device_service.rs",
        "fs_read_rooted",
        Category::RootBranch,
        Some("候选③：模块 root companion 提供特权读"),
    ),
    (
        "src/services/device_service.rs",
        "fs_run_rooted",
        Category::RootBranch,
        Some("候选③：模块 root companion 提供特权写"),
    ),
    (
        "src/services/device_service.rs",
        "hosted_chmod",
        Category::RootBranch,
        Some("候选③：模块 companion 提供特权写"),
    ),
    (
        "src/services/device_service.rs",
        "hosted_run_as_root",
        Category::RootBranch,
        Some("候选③：模块 companion 提供特权启动"),
    ),
    // 第八十五轮（root 支路的启动没包 su 的修复）：启动命令的形状从 `hosted_run_as_root`
    // 抽成这个纯构造函数，好让"到底包没包 su"变成可单测的事实而不是读代码读出来的印象。
    // 调用点没有增加，只是从上面那个函数搬了下来——登记必须跟着实现走，所以这里也搬。
    (
        "src/services/device_service.rs",
        "hosted_run_adb_args",
        Category::RootBranch,
        Some("候选③：模块 companion 提供特权启动"),
    ),
    // 起进程前把上一次的运行日志归档走（root 跑过一次会让日志变成 root:root 0644，
    // 下次以 shell 起就写不动，进程压根没 exec）。这一条 root 支路要跟着包 su，
    // 所以是**新增的一个 shell 调用点**：按纪律在下面把期望数从 5 提到 6。
    (
        "src/services/device_service.rs",
        "hosted_retire_log_adb_args",
        Category::RootBranch,
        Some("候选③：模块 companion 提供特权启动后，桌面不再自己收尾日志"),
    ),
    (
        "src/services/device_service.rs",
        "read_hosted_log",
        Category::RootBranch,
        Some("候选③：模块 companion 提供跨 uid 特权读"),
    ),
    // UI-6 第一层：root 支路的参数/stdin 走"落盘 + 固定模板按行还原"，
    // 收尾那条 rm -f 也是 shell（正常路径由模板自己删，这里只兜"推成功但没跑起来"）
    (
        "src/services/device_service.rs",
        "rm_carried_files",
        Category::RootBranch,
        Some("候选③：模块 companion 提供特权启动后，桌面不再自己拼这条"),
    ),
];

/// 期望的调用点总数（不是登记条目数：一个函数可能有 3 处调用）。
const EXPECTED_SITES_PER_CATEGORY: &[(Category, usize)] = &[
    // 11 → 12（AR12.5）：新增的是 `agent_bootstrap::is_running` 这条**只读**探测，
    // 用来在动手之前问清"设备上有没有在跑的 Agent"。多一个 shell 调用点必须在这里
    // 留一句话——和当年 AR7.6 少一个点要改数字是同一条纪律。
    // 12 → 13：新增 `invalidate_remote_artifact`（握手报版本不符时清掉设备侧副本再重装）。
    // 它只在自愈路径上出现，属于 Agent 自己的装机生命周期，所以记在 Bootstrap 里。
    (Category::Bootstrap, 13),
    // 5 → 6（P13）：新增 `tool_market::run_checked`——工具市场的一键安装（push + chmod +
    // 只读复核）。它是 adb 的文件传输本职，不是 Agent 能力的回退腿，所以归 Transport。
    (Category::Transport, 8),
    // 2 → 1（提交本次）：`raw_shell_task` 整条删掉了，剩下 `raw_logcat_task` 一条。
    // 数字往下走也要在这里说一句，别让"少了一个回退点"悄悄发生。
    (Category::RawTool, 1),
    // 8（原 9）：AR7.6 把 root 终止从 `adb shell su -c "kill -9 <pid>"` 改成走 Agent 的
    // 固定提权脚本（先核身份再发信号），桌面侧那个 shell 调用点因此消失 —— AR12 少一项可删的
    // Legacy。这里跟着下调，是为了让"少一个回退点"也变成会被机器看见的事实，而不是悄悄发生。
    (Category::LegacyFallback, 7),
    // 5 → 6（第八十五轮）：新增的是"起进程前归档旧日志"那一条（root 支路必须与启动同一身份）。
    // 同一个批次里把启动命令从 `hosted_run_as_root` 抽到 `hosted_run_adb_args`，调用点没变多，
    // 只是换了归属——数字要动就得在这里留一句话，跟当年 AR7.6 少一个点要改数是同一条纪律。
    // 6 → 7（本轮）：设备页文件写侧的 root 腿 `fs_run_rooted`。它不是漂移出来的裸 adb shell——
    // 路径先过 `fs_policy` 白名单（增删改只允许 /data/local/tmp 与 /sdcard 两棵树，root 也不放宽范围），
    // 干完回设备复核（复核读不到 = 不算成功），所以按 RootBranch 归类，删除阶段照旧挂候选③。
    // 7 → 8（本轮）：再加一条**读**侧 root 腿 `fs_read_rooted`。用户要的是"给了 root 就能看 /data"——
    // Agent 以 shell 身份跑，`/data` 连 opendir 都进不去，它只能如实报"读不到"，而旧接口把结果压成
    // 一个数组，界面于是把"读不到"画成了"这个目录是空的"。这条腿同样不是裸 adb shell：
    // 只读、不做任何写；列目录用 `cd 目录 && ls -lA -- .`（第七十一轮删掉的旧 Legacy 腿正是
    // 因为直接 `ls -lA <符号链接目录>` 会列出链接自身、造出 /sdcard/sdcard 假条目）。
    (Category::RootBranch, 8),
];

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|v| v.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// 去掉 `#[cfg(test)]` 模块、跳过定义行，返回 (函数名, 行号)。
///
/// 模块收尾按「第 0 列的 `}`」判定而不是数大括号：字符字面量里的 `'{'` / `'}'`
/// 会把计数骗过去（第一版就这么在 `agent_manager.rs` 里提前结束跳过，把测试腿
/// 当成生产调用点报了出来）。rustfmt 保证顶层项的收尾大括号在第 0 列，这个事实
/// 比词法分析便宜且可靠。
fn production_shell_sites(text: &str) -> Vec<(String, usize)> {
    let mut in_test_module = false;
    let mut pending_skip = false;
    let mut current = String::from("<mod>");
    let mut out: Vec<(String, usize)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if in_test_module {
            if line == "}" {
                in_test_module = false;
            }
            continue;
        }
        if pending_skip {
            if trimmed.starts_with("mod ") {
                in_test_module = true;
                pending_skip = false;
                continue;
            }
            pending_skip = false;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_skip = true;
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(start) = trimmed.find("fn ") {
            let name: String = trimmed[start + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                current = name;
            }
        }
        let is_hit = trimmed.contains("cmd_shell(")
            || trimmed.contains("\"shell\".into()")
            || trimmed.contains("adb_task(")
            // AR8.5：安装带上了"任务收尾回收临时解包目录"，改走带 cleanup 的同一条路；
            // 扫描器必须认这个名字，否则安装点会从审计里静默消失（这次就真的消失过一次）。
            || trimmed.contains("adb_task_with_cleanup(")
            || trimmed.contains("\"adb.shell\"")
            || trimmed.contains("\"adb.logcat\"");
        let is_definition = trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.contains("macro_rules!");
        if is_hit && !is_definition {
            out.push((current.clone(), index + 1));
        }
    }
    out
}

fn scan() -> Vec<(String, String, usize)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");
    let mut files = Vec::new();
    collect_rs(&src, &mut files);
    assert!(files.len() > 20, "扫描文件太少，路径可能不对: {src:?}");
    let mut hits = Vec::new();
    for file in files {
        // adb.rs 是 cmd_* 构造器本体（定义处），不是调用点
        if file.to_string_lossy().ends_with("adapters/adb.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        for (function, line) in production_shell_sites(&text) {
            hits.push((rel.clone(), function.to_owned(), line));
        }
    }
    hits
}

#[test]
fn every_production_shell_site_is_registered() {
    let registered: BTreeSet<(String, String)> = REGISTRY
        .iter()
        .map(|(file, function, _category, _stage)| ((*file).to_owned(), (*function).to_owned()))
        .collect();
    assert_eq!(
        registered.len(),
        REGISTRY.len(),
        "登记表里有重复条目：同一处调用被登记两次会让计数说谎"
    );
    let mut unregistered = BTreeSet::new();
    let mut by_category: Vec<(Category, usize)> = Vec::new();
    let mut counted: std::collections::HashMap<Category, usize> = Default::default();
    for (file, function, line) in scan() {
        if !registered.contains(&(file.clone(), function.clone())) {
            unregistered.insert(format!("{file}:{line} 在 {function}()"));
            continue;
        }
        let Some(entry) = REGISTRY
            .iter()
            .find(|(f, fn_name, _category, _stage)| *f == file && *fn_name == function)
        else {
            unreachable!("登记集合已确认包含 {file}::{function}")
        };
        *counted.entry(entry.2).or_insert(0) += 1;
    }
    assert!(
        unregistered.is_empty(),
        "未登记的 adb shell / 长任务调用点（新增业务能力必须先归类，见本文件头说明）:\n{}",
        unregistered.into_iter().collect::<Vec<_>>().join("\n")
    );
    for (category, expected) in EXPECTED_SITES_PER_CATEGORY {
        let actual = counted.get(category).copied().unwrap_or(0);
        by_category.push((*category, actual));
        // 未匹配到任何登记项的类别也要能显示出来，所以先把已有的都塞进 by_category
        let _ = &by_category;
        assert_eq!(
            actual, *expected,
            "{category:?} 类的调用点数量变了（{actual} ≠ {expected}）；实际分布 {by_category:?}"
        );
    }
}

#[test]
fn registry_entries_still_exist_and_legacy_entries_have_removal_stage() {
    let scanned: BTreeSet<(String, String)> = scan()
        .into_iter()
        .map(|(file, function, _line)| (file, function))
        .collect();
    for (file, function, category, stage) in REGISTRY {
        assert!(
            scanned.contains(&((*file).to_owned(), (*function).to_owned())),
            "登记表里的 {file}::{function} 已经没有 shell 调用了，条目要一起删掉（登记与实现不能分叉）"
        );
        if matches!(category, Category::LegacyFallback | Category::RootBranch) {
            assert!(
                stage.is_some(),
                "{file}::{function} 属 {category:?}，必须写明临时删除阶段"
            );
        }
    }
}
