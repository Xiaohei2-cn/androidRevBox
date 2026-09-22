//! TaskService：统一任务编排（P2）。
//! 职责：建任务记录 → spawn tokio worker 执行 ProcessService.execute →
//! 每行输出同时「事件推送 + 落库」→ 收敛终态并推 task://status → 清理取消令牌。
//! 命令返回 task_id 即结束，不在 IPC 上阻塞（长任务走事件流）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::Emitter;
use uuid::Uuid;

use crate::core::error::{CoreError, CoreResult};
use crate::core::ipc::{AppEvent, event_names};
use crate::db::{Db, task_repo};
use crate::models::task::{TaskDto, TaskLogDto};
use crate::services::process_service::{self, CancellationToken, CommandSpec, Outcome, StreamKind};

const MAX_NAME_LEN: usize = 200;

type Registry = Arc<Mutex<HashMap<String, CancellationToken>>>;

pub struct TaskService {
    db: Arc<Db>,
    app: tauri::AppHandle,
    /// 运行中任务的取消令牌：task_id → token。worker 结束后自清理。
    running: Registry,
}

impl TaskService {
    pub fn new(db: Arc<Db>, app: tauri::AppHandle) -> Self {
        // 进程异常退出遗留的 running/pending 统一标记 failed（重启恢复一致性）
        match task_repo::mark_orphans_failed(&db) {
            Ok(n) if n > 0 => tracing::warn!(orphans = n, "启动时清理孤儿任务"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "清理孤儿任务失败"),
        }
        Self {
            db,
            app,
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 起一个 shell/命令任务，返回 task_id（立即返回，不阻塞）。
    pub fn start(&self, spec: CommandSpec) -> CoreResult<String> {
        self.start_with_kind("shell", spec)
    }

    /// 指定 task_type 的任务（P10：frida 会话复用同一生命周期/事件流/日志回放，
    /// 仅 kind 不同——kind 参数化，不迁 schema）。
    pub fn start_with_kind(&self, kind: &str, spec: CommandSpec) -> CoreResult<String> {
        let name = build_task_name(&spec);
        self.start_with_kind_named(kind, &name, spec)
    }

    /// kind + 自定义可读任务名（frida 会话名「spawn com.x · hook.js」式）。
    pub fn start_with_kind_named(
        &self,
        kind: &str,
        name: &str,
        mut spec: CommandSpec,
    ) -> CoreResult<String> {
        let name = if name.trim().is_empty() {
            build_task_name(&spec)
        } else {
            truncate_name(name)
        };
        let id = Uuid::new_v4().to_string();
        task_repo::insert(&self.db, &id, kind, &name, "pending")?;

        let token = CancellationToken::default();
        self.lock_running().insert(id.clone(), token.clone());

        let db = self.db.clone();
        let app = self.app.clone();
        let registry = self.running.clone();
        let worker_id = id.clone();

        tauri::async_runtime::spawn(async move {
            // 先把回收点摘出来：spec 会被 execute 消耗掉
            let cleanup_paths = std::mem::take(&mut spec.cleanup_paths);
            let _ = task_repo::update(&db, &worker_id, "running", None, false);
            emit_status(&app, &worker_id, "running", None);

            let sink_db = db.clone();
            let sink_app = app.clone();
            let sink_id = worker_id.clone();
            let sink: process_service::Sink = Box::new(move |kind: StreamKind, line: &str| {
                // 落库失败不阻断泵送，仅记日志
                if let Err(e) = task_repo::append_log(&sink_db, &sink_id, kind.as_str(), line) {
                    tracing::error!(error = %e, task_id = %sink_id, "写任务日志失败");
                }
                let evt = AppEvent::new(
                    event_names::TASK_OUTPUT,
                    TaskOutputPayload {
                        task_id: sink_id.clone(),
                        stream: kind.as_str().to_string(),
                        chunk: line.to_string(),
                    },
                );
                if let Err(e) = sink_app.emit(evt.event, &evt) {
                    tracing::debug!(error = %e, "推送任务输出事件失败");
                }
            });

            let outcome = process_service::execute(spec, sink, token).await;

            let (status, exit_code) = match outcome {
                Ok(Outcome::Exited(code)) => {
                    let st = if code == Some(0) { "success" } else { "failed" };
                    (st, code)
                }
                Ok(Outcome::Cancelled) => ("cancelled", None),
                // 超时视同失败（退出码不可得），系统行已写入原因
                Ok(Outcome::TimedOut) => ("failed", None),
                Err(e) => {
                    tracing::error!(error = %e, task_id = %worker_id, "任务执行异常");
                    let _ = task_repo::append_log(
                        &db,
                        &worker_id,
                        StreamKind::System.as_str(),
                        &e.to_string(),
                    );
                    ("failed", None)
                }
            };

            let _ = task_repo::update(&db, &worker_id, status, exit_code, true);
            cleanup_task_paths(&cleanup_paths);
            emit_status(&app, &worker_id, status, exit_code);
            // 自清理：终态后令牌不再有用
            if let Ok(mut map) = registry.lock() {
                map.remove(&worker_id);
            }
        });

        Ok(id)
    }

    /// 取消运行中任务；对已结束/不存在的任务返回错误。
    pub fn cancel(&self, id: &str) -> CoreResult<()> {
        let token = self
            .lock_running()
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::Internal(format!("任务未在运行，无法取消: {id}")))?;
        token.cancel();
        Ok(())
    }

    pub fn list(&self, limit: i64) -> CoreResult<Vec<TaskDto>> {
        Ok(task_repo::list(&self.db, limit)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn logs(&self, id: &str, limit: i64) -> CoreResult<Vec<TaskLogDto>> {
        Ok(task_repo::logs(&self.db, id, limit)?
            .into_iter()
            .map(|(stream, chunk, ts)| TaskLogDto { stream, chunk, ts })
            .collect())
    }

    fn lock_running(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.running.lock().expect("task registry lock poisoned")
    }
}

/// task://output 事件 payload（协议字段，见 docs/ipc-conventions.md）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskOutputPayload {
    task_id: String,
    stream: String,
    chunk: String,
}

/// 回收发起方留下的临时路径（例如 `.apks` 解出来的散装 APK）。
///
/// 为什么必须挂在**任务收尾**而不是发起方：`adb install-multiple` 是异步任务，
/// 发起方拿到 task_id 就返回了，那时 adb 还在读这些文件；提前删等于把安装打断。
/// 反过来，成功/失败/取消三条路都得清，否则一次大包几百 MB 有去无回。
/// 本来还有第二道防线（`apk_bundle::sweep_stale_workdirs` 扫陈旧目录），
/// 但实测它会把**并行任务正在用的目录**当残留删掉（测试里两个任务同时跑就复现了），
/// 所以撤掉：宁可漏在崩溃时留一个目录，也不能悄悄删掉正在使用的文件。
pub(crate) fn cleanup_task_paths(paths: &[PathBuf]) {
    for path in paths {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(error) = removed
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %error, "任务临时路径回收失败");
        }
    }
}

fn emit_status(app: &tauri::AppHandle, id: &str, status: &str, exit_code: Option<i32>) {
    let evt = AppEvent::new(
        event_names::TASK_STATUS,
        serde_json::json!({ "taskId": id, "status": status, "exitCode": exit_code }),
    );
    if let Err(e) = app.emit(evt.event, &evt) {
        tracing::debug!(error = %e, "推送任务状态事件失败");
    }
}

/// 由 CommandSpec 生成可读任务名：executable 文件名 + 前几个参数，截断。
fn build_task_name(spec: &CommandSpec) -> String {
    let base = std::path::Path::new(&spec.executable)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| spec.executable.clone());
    let mut name = base;
    for arg in spec.args.iter().take(3) {
        name.push(' ');
        name.push_str(arg);
    }
    truncate_name(&name)
}

/// 按字节上限截断到字符边界（任务名统一收口）。
fn truncate_name(name: &str) -> String {
    let mut name = name.to_string();
    if name.len() > MAX_NAME_LEN {
        let mut cut = MAX_NAME_LEN;
        while cut > 0 && !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
    }
    name
}

#[cfg(test)]
mod tests {
    #[test]
    fn cleanup_task_paths_removes_dirs_files_and_tolerates_missing() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("bundle");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("base.apk");
        std::fs::write(&file, b"payload").unwrap();
        let single = root.path().join("loose.bin");
        std::fs::write(&single, b"x").unwrap();
        let gone = root.path().join("not-there");

        cleanup_task_paths(&[dir.clone(), single.clone(), gone.clone()]);

        assert!(!dir.exists(), "目录必须被回收");
        assert!(!single.exists(), "文件必须被回收");
        // 不存在不能报错：任务被取消时可能已经有一半被清掉了
        cleanup_task_paths(&[dir, single, gone]);
        cleanup_task_paths(&[]);
    }

    use super::*;

    #[test]
    fn task_name_uses_basename_and_truncates() {
        let spec = CommandSpec {
            cleanup_paths: Vec::new(),
            executable: "/bin/echo".into(),
            args: vec!["hello".into(), "world".into()],
            ..Default::default()
        };
        assert_eq!(build_task_name(&spec), "echo hello world");

        let long = CommandSpec {
            cleanup_paths: Vec::new(),
            executable: "/usr/bin/env".into(),
            args: vec!["x".repeat(500)],
            ..Default::default()
        };
        let name = build_task_name(&long);
        assert!(name.len() <= MAX_NAME_LEN);
    }
}
