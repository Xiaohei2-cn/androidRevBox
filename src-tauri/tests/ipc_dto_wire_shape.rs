//! IPC DTO 键名护栏（AR9.1 顺带发现的真实缺陷，值得单独钉住）。
//!
//! `commands::device::*` 里有一批响应**直接返回 `agent_protocol` 的 DTO**，
//! 而这个协议的字段线格式是 snake_case；App 内部其它 IPC 模型（`models/*`、
//! `services/zygisk_applist.rs`）却统一标了 `rename_all = "camelCase"`。
//! 前端照 App 习惯手抄类型，于是 `stat.modeText`、`result.targetPath`、
//! `record.startTimeTicks` 这类读法在运行时全是 `undefined`——typecheck、
//! 前端单测、真机腿都不会红，只有眼睛看界面才发现（文件页的权限位与 mtime
//! 就是这样空了好几个阶段，AR8.4 的 SO 页也踩了同一坑）。
//!
//! 这里把两侧钉在一起：Rust 侧序列化出来的**真实多词键名**必须逐个出现在
//! `src/api/device.ts` 对应的接口里。任何一方改名而另一方不改，直接红。

use std::collections::BTreeSet;
use std::path::PathBuf;

use agent_protocol::{
    FileKind, FileStat, FilesystemPreviewResult, FilesystemStatResult, FridaServerStartResult,
    FridaServerState, FridaServerStatusResult, FridaServerStopResult, HostedRunRecord,
    HostedRunState, HostedStopResult, KillOutcome, OperationStep, PackageUninstallResult,
    PackageWriteAction, PackageWriteResult, PreviewEncoding, ReplaceNativeLibraryResult,
    WriteOutcome,
};
use serde_json::Value;

fn steps() -> Vec<OperationStep> {
    vec![OperationStep {
        name: "install".into(),
        ok: true,
        detail: None,
    }]
}

fn file_stat() -> FileStat {
    FileStat {
        name: "a".into(),
        kind: FileKind::File,
        mode: 0o644,
        mode_text: "-rw-r--r--".into(),
        uid: 0,
        gid: 0,
        size: 1,
        mtime_unix: 2,
        symlink_target: None,
        readable: true,
    }
}

fn run_record() -> HostedRunRecord {
    HostedRunRecord {
        handle: "h".into(),
        name: "toybox".into(),
        pid: 1,
        start_time_ticks: 2,
        started_at_unix: 3,
        log_path: "/data/local/tmp/x.log".into(),
        root: false,
        state: HostedRunState::Running,
        exit_code: None,
        detail: None,
    }
}

/// 每个跨 IPC 的 DTO：`(前端接口名, Rust 值, 期望的多词键名)`。
/// 前端接口名与 Rust 类型名不同的地方显式写出来（历史上就是这种别名最容易漏）。
fn cases() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        (
            "FileStat",
            serde_json::to_value(file_stat()).unwrap(),
            vec!["mode_text", "mtime_unix"],
        ),
        (
            "FileStatResult",
            serde_json::to_value(FilesystemStatResult {
                requested_path: "/a".into(),
                path: "/a".into(),
                stat: file_stat(),
            })
            .unwrap(),
            vec!["requested_path"],
        ),
        (
            "FilePreviewResult",
            serde_json::to_value(FilesystemPreviewResult {
                path: "/a".into(),
                size: 1,
                offset: 0,
                returned_bytes: 1,
                encoding: PreviewEncoding::Utf8,
                text: Some("x".into()),
                hex: None,
                truncated: false,
                detail: None,
            })
            .unwrap(),
            vec!["returned_bytes"],
        ),
        (
            "HostedRunRecord",
            serde_json::to_value(run_record()).unwrap(),
            vec!["start_time_ticks", "started_at_unix", "log_path"],
        ),
        (
            "HostedStopResult",
            serde_json::to_value(HostedStopResult {
                record: run_record(),
                outcome: KillOutcome::Signaled,
                identity_verified: true,
                record_dropped: true,
            })
            .unwrap(),
            vec!["identity_verified", "record_dropped"],
        ),
        (
            "ReplaceNativeLibraryResult",
            serde_json::to_value(ReplaceNativeLibraryResult {
                package: "com.x".into(),
                target_path: "/data/app/x/lib/arm64/a.so".into(),
                staged_path: "/data/local/tmp/app-reverse-tools-so/o/a.so".into(),
                operation_id: "op".into(),
                outcome: WriteOutcome::Executed,
                verified: true,
                replaced_existing: false,
                steps: steps(),
                rolled_back: None,
                backup_path: None,
                detail: None,
            })
            .unwrap(),
            vec![
                "target_path",
                "staged_path",
                "operation_id",
                "replaced_existing",
            ],
        ),
        (
            "PackageWriteResult",
            serde_json::to_value(PackageWriteResult {
                action: PackageWriteAction::Launch,
                package: "com.x".into(),
                operation_id: "op".into(),
                outcome: WriteOutcome::Executed,
                verified: true,
                pid: Some(1),
                detail: None,
                ran_as_root: false,
            })
            .unwrap(),
            vec!["operation_id", "ran_as_root"],
        ),
        (
            "PackageUninstallResult",
            serde_json::to_value(PackageUninstallResult {
                package: "com.x".into(),
                operation_id: "op".into(),
                outcome: WriteOutcome::Executed,
                verified: true,
                keep_data: false,
                steps: steps(),
                detail: None,
            })
            .unwrap(),
            vec!["operation_id", "keep_data"],
        ),
        (
            "FridaServerStatus",
            serde_json::to_value(FridaServerStatusResult {
                state: FridaServerState::RunningAsRoot,
                running: true,
                as_root: true,
                binary_name: "frida-server".into(),
                pid: Some(1),
                uid: Some(0),
                listen_address: Some("127.0.0.1".into()),
                port: Some(27042),
                listening: true,
                version: None,
                detail: None,
            })
            .unwrap(),
            vec!["binary_name", "listen_address", "as_root"],
        ),
        (
            "FridaServerStartResult",
            serde_json::to_value(FridaServerStartResult {
                operation_id: "op".into(),
                outcome: WriteOutcome::Executed,
                verified: true,
                binary_name: "frida-server".into(),
                bind: "127.0.0.1".into(),
                port: 27042,
                pid: Some(1),
                uid: Some(0),
                version: None,
                steps: steps(),
                detail: None,
            })
            .unwrap(),
            vec!["operation_id", "binary_name"],
        ),
        (
            "FridaServerStopResult",
            serde_json::to_value(FridaServerStopResult {
                operation_id: "op".into(),
                outcome: WriteOutcome::Executed,
                verified: true,
                binary_name: "frida-server".into(),
                pid: Some(1),
                uid: Some(0),
                steps: steps(),
                detail: None,
            })
            .unwrap(),
            vec!["operation_id", "binary_name"],
        ),
    ]
}

#[test]
fn every_cross_ipc_dto_uses_snake_case_multi_word_keys() {
    for (interface, value, expected) in cases() {
        let actual: BTreeSet<String> = value
            .as_object()
            .unwrap_or_else(|| panic!("{interface} 必须序列化成对象"))
            .keys()
            .filter(|key| key.contains('_'))
            .cloned()
            .collect();
        let want: BTreeSet<String> = expected.iter().map(|key| (*key).to_owned()).collect();
        assert_eq!(
            actual, want,
            "{interface} 的多词键名与预期不符（改了协议就同步改测试与前端）"
        );
    }
}

#[test]
fn frontend_interfaces_declare_exactly_those_keys() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/api/device.ts");
    let text = std::fs::read_to_string(&path).expect("读 src/api/device.ts 失败");
    for (interface, _value, expected) in cases() {
        let head = format!("export interface {interface}");
        let start = text
            .find(&head)
            .unwrap_or_else(|| panic!("前端缺少 {interface} 的类型声明"));
        let rest = &text[start..];
        let body = &rest[..rest.find("\n}").unwrap_or(rest.len())];
        for field in expected {
            assert!(
                body.contains(&format!("{field}:")) || body.contains(&format!("{field}?:")),
                "{interface}.{field}：wire 上是 {field}，前端类型里没有它（历史上就是这里对不上）"
            );
            let camel = field.replace('_', "");
            assert!(
                !body.contains(&format!("{camel}:")) && !body.contains(&format!("{camel}?:")),
                "{interface} 里还留着 camelCase 变体 {camel}，两侧命名又分叉了"
            );
        }
    }
}

/// `device_binary_list` 是唯一**没有**裸传协议 DTO 的一条：它在边界上把
/// `HostedBinaryInfo`（snake_case）映射成 `adapters::adb::HostedBinary`
/// （`rename_all = "camelCase"`），前端因此可以一直用 App 的 camelCase 习惯。
/// 这条测试的存在不是为了证明「裸传没问题」，而是把另外 11 条待迁移的现实钉住：
/// 将来给它们补映射层时，删掉对应 case 的同时也要删掉 device.ts 里的 snake_case 声明。
#[test]
fn hosted_binary_list_is_the_mapped_boundary_and_stays_camel_case() {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/api/device.ts"),
    )
    .expect("读 src/api/device.ts 失败");
    let start = text
        .find("export interface HostedBinary ")
        .expect("HostedBinary 接口必须存在");
    let rest = &text[start..];
    let body = &rest[..rest.find("\n}").unwrap_or(rest.len())];
    assert!(
        body.contains("hasExec:") && body.contains("perms:"),
        "托管列表走映射层，前端必须是 camelCase 且字段名不变：{body}"
    );
    assert!(
        !body.contains("has_exec"),
        "HostedBinary 里不该出现 wire 名，说明有人把映射层拆了"
    );
}
