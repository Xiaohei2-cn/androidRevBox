//! 任务命令层（P2）：校验参数 → TaskService 编排 → 返回 task_id。
//! 流式输出一律走事件（task://output / task://status），命令本身不阻塞。

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::models::task::{TaskDto, TaskLogDto};
use crate::services::process_service::CommandSpec;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunArgs {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// 超时毫秒；空 = 不限
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[tauri::command]
pub fn task_run(state: tauri::State<'_, AppState>, args: TaskRunArgs) -> CoreResult<String> {
    if args.executable.trim().is_empty() {
        return Err(CoreError::Internal("executable 不能为空".to_string()));
    }
    if let Some(t) = args.timeout_ms {
        if !(100..=3_600_000).contains(&t) {
            return Err(CoreError::Internal(
                "timeout 需在 100ms–1h 之间".to_string(),
            ));
        }
    }
    let spec = CommandSpec {
        executable: args.executable,
        args: args.args,
        cwd: args.cwd,
        timeout: args.timeout_ms.map(Duration::from_millis),
        env_extra: args.env,
    };
    state.task.start(spec)
}

#[tauri::command]
pub fn task_cancel(state: tauri::State<'_, AppState>, id: String) -> CoreResult<()> {
    state.task.cancel(&id)
}

#[tauri::command]
pub fn task_list(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
) -> CoreResult<Vec<TaskDto>> {
    state.task.list(limit.unwrap_or(100))
}

#[tauri::command]
pub fn task_logs(
    state: tauri::State<'_, AppState>,
    id: String,
    limit: Option<i64>,
) -> CoreResult<Vec<TaskLogDto>> {
    state.task.logs(&id, limit.unwrap_or(1000))
}
