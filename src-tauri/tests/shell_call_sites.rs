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
        "src/adapters/agent_bootstrap.rs",
        "remove_remote_file",
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
        "src/services/device_service.rs",
        "replace_native_library",
        Category::Transport,
        None,
    ),
    // Raw Tool：AR9.3 已限定只能由命令层为「用户主动发起的会话」调用
    (
        "src/services/device_service.rs",
        "raw_shell_task",
        Category::RawTool,
        None,
    ),
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
    (
        "src/services/device_service.rs",
        "hosted_kill_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR7.2"),
    ),
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
    (
        "src/services/device_service.rs",
        "pkg_lib_dir_legacy",
        Category::LegacyFallback,
        Some("AR12.1 after AR8.3"),
    ),
    (
        "src/services/env_service.rs",
        "shell",
        Category::LegacyFallback,
        Some("AR12.1 after AR6.1"),
    ),
    // root=true 支路：做不到 typed 化，等候选③（模块 root companion）
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
    (
        "src/services/device_service.rs",
        "read_hosted_log",
        Category::RootBranch,
        Some("候选③：模块 companion 提供跨 uid 特权读"),
    ),
];

/// 期望的调用点总数（不是登记条目数：一个函数可能有 3 处调用）。
const EXPECTED_SITES_PER_CATEGORY: &[(Category, usize)] = &[
    (Category::Bootstrap, 11),
    (Category::Transport, 4),
    (Category::RawTool, 2),
    (Category::LegacyFallback, 9),
    (Category::RootBranch, 4),
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
