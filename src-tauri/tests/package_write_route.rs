//! AR8.1 边界测试：三个包写操作（启动 / 强停 / 卸载）只能走 Agent typed 通道。
//!
//! 用户口径已定（D041）：这三类操作**不留任务卡**，改由 typed 结果直接回用户。
//! 「不产卡」这件事本身没法用单元测试证明（少了 `TaskService` 调用就少了一张卡，
//! 但漏调一次路由也不会让测试变红），所以这里用源码扫描把三条硬事实钉住：
//! ① 服务层不再为这三个动作生成 `adb.launch/force_stop/uninstall` 任务；
//! ② 三个方法都必须先过 `require_agent_write_route`（写操作不自动回退，§3.6）；
//! ③ command 层返回 typed 结果类型，而不是任务 id 字符串。
//! 谁要改回去，就得同时改这个测试——那正是要人确认的时刻。

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("读 {rel:?} 失败: {error}"))
}

/// 去掉注释行：本测试钉的是**代码事实**，文档里提到旧写法（`adb_task("adb.launch")`
/// 作为对照说明）不算调用点。与 AR9.3 的 `raw_tool_boundary` 同一口径。
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 取出某个 `pub async fn <name>(` 到下一个同级函数为止的函数体（足够覆盖单条规则）。
fn function_body(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("找不到 {signature}，实现被改名或删除了"));
    let rest = &source[start..];
    let tail = rest[signature.len()..]
        .find("\n    pub async fn ")
        .or_else(|| rest[signature.len()..].find("\n    pub fn "))
        .map(|index| index + signature.len())
        .unwrap_or(rest.len());
    rest[..tail].to_string()
}

#[test]
fn package_writes_go_agent_and_never_produce_adb_tasks() {
    let service = strip_comments(&read("src/services/device_service.rs"));
    for kind in ["adb.launch", "adb.force_stop", "adb.uninstall"] {
        assert!(
            !service.contains(&format!("\"{kind}\"")),
            "{kind} 已经改由 Agent typed 执行，服务层不该再出现这个任务类型"
        );
    }

    for (signature, method) in [
        ("pub async fn launch(", "ACTIVITY_LAUNCH"),
        ("pub async fn force_stop(", "ACTIVITY_FORCE_STOP"),
        ("pub async fn uninstall(", "PACKAGE_UNINSTALL"),
    ] {
        let body = function_body(&service, signature);
        assert!(
            body.contains(method),
            "{signature}.. 必须请求 {method}，实际函数体开头: {}",
            body.chars().take(120).collect::<String>()
        );
        assert!(
            body.contains("require_agent_write_route"),
            "{signature}.. 是写操作，必须先过 Agent 路由把关（不回退）"
        );
        assert!(
            body.contains("audit_package_write"),
            "{signature}.. 必须写审计事件（§3.7）"
        );
        assert!(
            !body.contains("adb_task") && !body.contains("tasks.start"),
            "{signature}.. 不能再产任务卡（用户口径：不留卡）"
        );
    }
}

#[test]
fn package_write_commands_return_typed_results_not_task_ids() {
    let commands = strip_comments(&read("src/commands/device.rs"));
    for (signature, result_type) in [
        (
            "pub async fn device_launch(",
            "CoreResult<PackageWriteResult>",
        ),
        (
            "pub async fn device_force_stop(",
            "CoreResult<PackageWriteResult>",
        ),
        (
            "pub async fn device_uninstall(",
            "CoreResult<PackageUninstallResult>",
        ),
    ] {
        let body = function_body(&commands, signature);
        assert!(
            body.contains(result_type),
            "{signature}.. 必须返回 {result_type}，实际: {}",
            body.chars().take(160).collect::<String>()
        );
    }
}
