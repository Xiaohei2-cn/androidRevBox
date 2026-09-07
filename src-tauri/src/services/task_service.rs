//! TaskService：统一任务编排（P2）。
//! 职责：建任务记录 → spawn tokio worker 执行 ProcessService.execute →
//! 每行输出同时「事件推送 + 落库」→ 收敛终态并推 task://status → 清理取消令牌。
//! 命令返回 task_id 即结束，不在 IPC 上阻塞（长任务走事件流）。

use std::collections::HashMap;
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
        let name = build_task_name(&spec);
        let id = Uuid::new_v4().to_string();
        task_repo::insert(&self.db, &id, "shell", &name, "pending")?;

        let token = CancellationToken::default();
        self.lock_running().insert(id.clone(), token.clone());

        let db = self.db.clone();
        let app = self.app.clone();
        let registry = self.running.clone();
        let worker_id = id.clone();

        tauri::async_runtime::spawn(async move {
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
    use super::*;

    #[test]
    fn task_name_uses_basename_and_truncates() {
        let spec = CommandSpec {
            executable: "/bin/echo".into(),
            args: vec!["hello".into(), "world".into()],
            cwd: None,
            timeout: None,
            env_extra: HashMap::new(),
        };
        assert_eq!(build_task_name(&spec), "echo hello world");

        let long = CommandSpec {
            executable: "/usr/bin/env".into(),
            args: vec!["x".repeat(500)],
            cwd: None,
            timeout: None,
            env_extra: HashMap::new(),
        };
        let name = build_task_name(&long);
        assert!(name.len() <= MAX_NAME_LEN);
    }
}
