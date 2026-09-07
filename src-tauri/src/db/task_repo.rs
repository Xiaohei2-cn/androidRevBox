//! tasks / task_logs 表访问层（P2）。

use rusqlite::params;

use crate::core::error::{CoreError, CoreResult};
use crate::db::Db;

/// 任务行（仓储级结构，向 DTO 的转换在 models/task.rs）
#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: String,
    pub task_type: String,
    pub name: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

pub const TASK_STATUSES: [&str; 5] = ["pending", "running", "success", "failed", "cancelled"];

pub fn is_valid_status(s: &str) -> bool {
    TASK_STATUSES.contains(&s)
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: r.get(0)?,
        task_type: r.get(1)?,
        name: r.get(2)?,
        status: r.get(3)?,
        exit_code: r.get(4)?,
        created_at: r.get(5)?,
        finished_at: r.get(6)?,
    })
}

const SELECT_COLS: &str = "id, type, name, status, exit_code, created_at, finished_at";

pub fn insert(db: &Db, id: &str, task_type: &str, name: &str, status: &str) -> CoreResult<()> {
    let (id, task_type, name, status) = (
        id.to_string(),
        task_type.to_string(),
        name.to_string(),
        status.to_string(),
    );
    db.with(move |conn| {
        conn.execute(
            "INSERT INTO tasks (id, type, name, status) VALUES (?1, ?2, ?3, ?4)",
            params![id, task_type, name, status],
        )?;
        Ok(())
    })
}

pub fn update(
    db: &Db,
    id: &str,
    status: &str,
    exit_code: Option<i32>,
    finished: bool,
) -> CoreResult<()> {
    if !is_valid_status(status) {
        return Err(CoreError::Internal(format!("非法任务状态: {status}")));
    }
    let id = id.to_string();
    let status = status.to_string();
    db.with(move |conn| {
        let n = conn.execute(
            "UPDATE tasks SET status = ?2, exit_code = COALESCE(?3, exit_code), \
             finished_at = CASE WHEN ?4 THEN strftime('%s','now') ELSE finished_at END \
             WHERE id = ?1",
            params![id, status, exit_code, finished],
        )?;
        if n == 0 {
            return Err(CoreError::Internal(format!("任务不存在: {id}")));
        }
        Ok(())
    })
}

pub fn append_log(db: &Db, task_id: &str, stream: &str, chunk: &str) -> CoreResult<()> {
    let (task_id, stream, chunk) = (task_id.to_string(), stream.to_string(), chunk.to_string());
    db.with(move |conn| {
        conn.execute(
            "INSERT INTO task_logs (task_id, stream, chunk) VALUES (?1, ?2, ?3)",
            params![task_id, stream, chunk],
        )?;
        Ok(())
    })
}

/// 任务列表（最新在前）。limit<=0 时默认 100。
pub fn list(db: &Db, limit: i64) -> CoreResult<Vec<TaskRow>> {
    let limit = if limit <= 0 { 100 } else { limit.min(500) };
    db.with(move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM tasks ORDER BY created_at DESC, rowid DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit], row_from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    })
}

/// 某任务日志（时间序）。
pub fn logs(db: &Db, task_id: &str, limit: i64) -> CoreResult<Vec<(String, String, i64)>> {
    let (task_id, limit) = (
        task_id.to_string(),
        if limit <= 0 { 1000 } else { limit.min(5000) },
    );
    db.with(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT stream, chunk, ts FROM task_logs WHERE task_id = ?1 \
             ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![task_id, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    })
}

/// 孤儿恢复：进程异常退出后把 running/pending 标记为 failed（启动时一次性）。
pub fn mark_orphans_failed(db: &Db) -> CoreResult<usize> {
    db.with(|conn| {
        let n = conn.execute(
            "UPDATE tasks SET status='failed', finished_at = strftime('%s','now') \
             WHERE status IN ('running','pending')",
            [],
        )?;
        Ok(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn db() -> Db {
        Db::in_memory().unwrap()
    }

    #[test]
    fn insert_update_list_roundtrip() {
        let d = db();
        insert(&d, "t1", "shell", "echo 测试", "pending").unwrap();
        update(&d, "t1", "running", None, false).unwrap();
        update(&d, "t1", "success", Some(0), true).unwrap();
        let rows = list(&d, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "success");
        assert_eq!(rows[0].exit_code, Some(0));
        assert!(rows[0].finished_at.is_some());
    }

    #[test]
    fn invalid_status_rejected() {
        let d = db();
        insert(&d, "t1", "shell", "n", "pending").unwrap();
        assert!(update(&d, "t1", "exploded", None, false).is_err());
    }

    #[test]
    fn update_missing_task_errors() {
        let d = db();
        assert!(update(&d, "ghost", "running", None, false).is_err());
    }

    #[test]
    fn logs_ordered_and_scoped() {
        let d = db();
        insert(&d, "t1", "shell", "n", "pending").unwrap();
        insert(&d, "t2", "shell", "n", "pending").unwrap();
        append_log(&d, "t1", "stdout", "a").unwrap();
        append_log(&d, "t1", "stderr", "b").unwrap();
        append_log(&d, "t2", "stdout", "x").unwrap();
        let l = logs(&d, "t1", 100).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1, "a");
        assert_eq!(l[1].0, "stderr");
    }

    #[test]
    fn orphans_marked_failed() {
        let d = db();
        insert(&d, "t1", "shell", "n", "running").unwrap();
        insert(&d, "t2", "shell", "n", "pending").unwrap();
        insert(&d, "t3", "shell", "n", "success").unwrap();
        let n = mark_orphans_failed(&d).unwrap();
        assert_eq!(n, 2);
        let rows = list(&d, 10).unwrap();
        let t3 = rows.iter().find(|r| r.id == "t3").unwrap();
        assert_eq!(t3.status, "success"); // 已完成任务不受影响
    }
}
