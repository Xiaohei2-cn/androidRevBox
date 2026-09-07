//! 任务相关 DTO。

use serde::Serialize;

use crate::db::task_repo::TaskRow;

/// 任务列表行（task_list 返回）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub id: String,
    pub task_type: String,
    pub name: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

impl From<TaskRow> for TaskDto {
    fn from(r: TaskRow) -> Self {
        Self {
            id: r.id,
            task_type: r.task_type,
            name: r.name,
            status: r.status,
            exit_code: r.exit_code,
            created_at: r.created_at,
            finished_at: r.finished_at,
        }
    }
}

/// 任务日志条目（task_logs 返回）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogDto {
    pub stream: String,
    pub chunk: String,
    pub ts: i64,
}
