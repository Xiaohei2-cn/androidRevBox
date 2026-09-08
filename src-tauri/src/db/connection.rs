//! 数据库连接与 migration。
//! rusqlite 是同步驱动，这里用 Mutex<Connection> 包一层（P1 的读写量下足够；
//! 若任务日志高频写入出现瓶颈，P2 再换 r2d2 连接池 + WAL）。

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::core::error::{CoreError, CoreResult};

/// 迁移版本从 1 递增；001 建表 SQL 自身幂等（IF NOT EXISTS），
/// 002 起为版本化 ALTER——重复执行安全由 rusqlite_migration 的 user_version 保证。
const MIGRATIONS: [&str; 3] = [
    include_str!("../../../migrations/001_init.sql"),
    include_str!("../../../migrations/002_tasks_display.sql"),
    include_str!("../../../migrations/003_plugins_transport.sql"),
];

fn build_migrations() -> Migrations<'static> {
    Migrations::new(MIGRATIONS.iter().copied().map(M::up).collect())
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// 打开（或创建）app data 目录下的数据库并执行增量 migration。
    pub fn connect(app_data_dir: &Path) -> CoreResult<Self> {
        std::fs::create_dir_all(app_data_dir)?;
        let db_path = app_data_dir.join("app-reverse-tools.db");
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;

        let ms = build_migrations();
        let mut conn = conn;
        ms.to_latest(&mut conn)
            .map_err(|e| CoreError::Internal(format!("database migration failed: {e}")))?;

        tracing::info!(path = %db_path.display(), "database ready");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 用于单元测试的内存库。
    #[cfg(test)]
    pub fn in_memory() -> CoreResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let ms = build_migrations();
        ms.to_latest(&mut conn)
            .map_err(|e| CoreError::Internal(format!("migration failed: {e}")))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub(crate) fn with<T>(&self, f: impl FnOnce(&Connection) -> CoreResult<T>) -> CoreResult<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CoreError::Internal("database connection lock poisoned".to_string()))?;
        f(&conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_from_zero_creates_all_tables() {
        let db = Db::in_memory().expect("migrate 0→1");
        let count: i64 = db
            .with(|c| {
                let mut stmt = c.prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN (\
                     'app_settings','devices','tasks','task_logs','plugins','algorithms','recent_commands','presets')",
                )?;
                Ok(stmt.query_row([], |r| r.get(0))?)
            })
            .expect("count tables");
        assert_eq!(count, 8, "八张表应全部建出");
    }

    #[test]
    fn migration_is_repeatable() {
        // 同一连接重复执行 to_latest 应无副作用（幂等回测）
        let mut conn = Connection::open_in_memory().unwrap();
        build_migrations().to_latest(&mut conn).expect("first run");
        build_migrations().to_latest(&mut conn).expect("second run");
    }
}
