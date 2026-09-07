-- 002: tasks 表补充展示字段与退出码（P2 任务系统）。
-- 幂等性依赖 rusqlite_migration 的 user_version 版本跟踪：每段只执行一次；
-- ALTER TABLE ADD COLUMN 无 IF NOT EXISTS 语法，属版本化迁移的正常用法。
ALTER TABLE tasks ADD COLUMN name TEXT NOT NULL DEFAULT '';
ALTER TABLE tasks ADD COLUMN exit_code INTEGER;
