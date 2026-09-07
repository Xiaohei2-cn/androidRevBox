//! SQLite 持久层：连接管理、migration runner、repository。
//! 约束：结构变更只走 migrations/*.sql，禁止启动时隐式改表。

pub mod config_repo;
pub mod connection;
pub mod task_repo;

pub use connection::Db;
