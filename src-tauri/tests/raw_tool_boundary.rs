//! AR9.3 边界测试：raw shell / raw logcat 只能由命令层为「用户主动发起的会话」调用。
//!
//! 阶段文档 §2.3 把「用户原始 Shell」和 `adb logcat` 明确留在 Desktop ADB 侧
//! （它们本来就是传输/调试工具），但同一份文档要求**业务 Service 不得复用**这两个入口，
//! 否则「前台应用检测」「包列表」这类功能会偷偷退化回字符串拼 shell，
//! AR5~AR8 建立的 typed 边界就白做。Rust 的 `pub(in ...)` 不能跨模块分支限制可见性，
//! 所以这条规则用源码扫描钉住：新增调用点必须改测试，改测试就是要人确认它是合法用途。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 需要保护的入口（方法名带 `_task`，产出 TaskService 任务而不是业务结果）。
const RAW_ENTRIES: &[&str] = &["raw_shell_task(", "raw_logcat_task("];
/// 唯一允许调用它们的位置（相对 crate 根）。
const ALLOWED_PREFIX: &str = "src/commands/";

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

#[test]
fn raw_shell_and_logcat_entries_are_only_called_by_the_command_layer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");
    let mut files = Vec::new();
    collect_rs(&src, &mut files);
    assert!(
        files.len() > 10,
        "扫描到的源文件太少（{:#?}），路径可能不对",
        src
    );

    let mut offenders: BTreeSet<String> = BTreeSet::new();
    let mut call_sites = 0_usize;
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let rel = file.strip_prefix(&root).unwrap_or(file.as_path());
        let rel_text = rel.display().to_string();
        let allowed = rel_text.starts_with(ALLOWED_PREFIX);
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for entry in RAW_ENTRIES {
                // 定义处（`pub async fn raw_*(`）跳过，只看调用点
                let is_definition = trimmed.contains("fn ") && trimmed.contains(entry);
                if !trimmed.contains(entry) || is_definition {
                    continue;
                }
                call_sites += 1;
                if !allowed {
                    offenders.insert(format!("{rel_text}:{}", index + 1));
                }
            }
        }
    }
    assert!(
        call_sites >= 2,
        "一个调用点都没扫到，说明扫描或命名已失效，这个测试就别留着假绿灯"
    );
    assert!(
        offenders.is_empty(),
        "raw shell/logcat 入口被命令层之外调用了：{offenders:?}\n\
         §2.3 允许它们作为用户主动发起的原始工具，但业务 Service 必须走 typed Agent API；\
         如果确实新增了一个合法的原始工具入口，请连同本文件与阶段文档 §2.3/AR9.3 一起更新。"
    );
}
