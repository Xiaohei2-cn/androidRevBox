-- 001: 初始 schema（总案 §10 八张表）。全部 IF NOT EXISTS，配合 rusqlite_migration 版本管理实现幂等。
CREATE TABLE IF NOT EXISTS app_settings (
  key        TEXT PRIMARY KEY NOT NULL,
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE TABLE IF NOT EXISTS devices (
  serial    TEXT PRIMARY KEY NOT NULL,
  transport TEXT NOT NULL DEFAULT 'usb',
  name      TEXT NOT NULL DEFAULT '',
  last_seen INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE TABLE IF NOT EXISTS tasks (
  id          TEXT PRIMARY KEY NOT NULL,
  type        TEXT NOT NULL,
  status      TEXT NOT NULL CHECK (status IN ('pending', 'running', 'success', 'failed', 'cancelled')),
  created_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
  finished_at INTEGER
);

CREATE TABLE IF NOT EXISTS task_logs (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  stream  TEXT NOT NULL CHECK (stream IN ('stdout', 'stderr', 'system')),
  chunk   TEXT NOT NULL,
  ts      INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_task_logs_task ON task_logs(task_id);

CREATE TABLE IF NOT EXISTS plugins (
  id      TEXT PRIMARY KEY NOT NULL,
  version TEXT NOT NULL,
  type    TEXT NOT NULL,
  path    TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  abi     INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS algorithms (
  plugin_id       TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  id              TEXT NOT NULL,
  descriptor_json TEXT NOT NULL,
  PRIMARY KEY (plugin_id, id)
);

CREATE TABLE IF NOT EXISTS recent_commands (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  command TEXT NOT NULL,
  args    TEXT NOT NULL DEFAULT '[]',
  cwd     TEXT NOT NULL DEFAULT '',
  used_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE TABLE IF NOT EXISTS presets (
  id           TEXT PRIMARY KEY NOT NULL,
  type         TEXT NOT NULL,
  name         TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
