-- 003 (P6): plugins 表补充 transport 列（in-process | process）。
-- 历史行均为 in-process（P4 只有 C ABI 动态库），默认值即兼容旧数据。
-- 注意：ALTER ADD COLUMN 无 IF NOT EXISTS，重复执行安全由 rusqlite_migration 的 user_version 保证。
ALTER TABLE plugins ADD COLUMN transport TEXT NOT NULL DEFAULT 'in-process';
