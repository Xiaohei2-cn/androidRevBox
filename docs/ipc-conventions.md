# 前后端通信规范（P1 定稿）

> 适用范围：Tauri Command、事件流、DTO、错误、配置键、数据库。
> 实现锚点：`src-tauri/src/core/error.rs`、`core/ipc.rs`、`src/api/`。

## 1. 命令（Command）

- 命名：`域_动作`，全小写下划线（`config_snapshot`、`log_set_level`、`system_ping`）。
- 前端**唯一** invoke 出口是 `src/api/`（域封装：`api/client.ts` 收口 `invoke`）；组件/hooks 不得直接 invoke。
- Command 层只做参数校验与 Service 转发；业务规则一律在 `services/`。
- 返回类型：`CoreResult<T>`（即 `Result<T, CoreError>`），无 `IpcResult` 包装层——Tauri 对 Err 走 reject，错误体见 §3。

## 2. DTO 命名

- Rust 侧 `#[serde(rename_all = "camelCase")]`；前端 TS interface 一律 camelCase，与后端逐字段对齐（例：`appVersion`、`pluginId`、`lastSeen`）。
- 跨层数据必须经 `models/` 的 DTO，禁止把内部结构直接序列化给前端。

## 3. 错误形状

`CoreError` 序列化为结构化对象：

```json
{ "code": "DATABASE", "message": "数据库错误: ..." }
```

| code | 触发层 | 说明 |
|---|---|---|
| `IO` | 文件/进程/网络 | `std::io::Error` 转换 |
| `SERIALIZATION` | JSON | `serde_json::Error` 转换 |
| `DATABASE` | rusqlite | `rusqlite::Error` 转换 |
| `INTERNAL` | Service 校验 | 非法键/值、业务约束拒绝 |

- 前端 catch 后按 `code` 分支、按 `message` 展示（`message` 含中文可读上下文）。
- 错误必须可定位：日志里用 `tracing` 记录完整错误链，跨层上下文用 `anyhow`。

## 4. 事件流（Event Channel）

- 长任务统一「命令返回 task_id + 事件流」，禁止长阻塞 IPC。
- 封装：`AppEvent<P>`（`src-tauri/src/core/ipc.rs`），形状 `{ event, timestamp, payload }`，timestamp 为 Unix 秒。
- 事件名注册表 `event_names`（字符串只定义一次，前端从这里引同名常量）：

| 事件名 | 启用阶段 | payload（约定） |
|---|---|---|
| `task://output` | P2 | `{ taskId, stream: "stdout"\|"stderr"\|"system", chunk, ts }` |
| `task://status` | P2 | `{ taskId, status, exitCode?, finishedAt }` |
| `device://changed` | P3 | `{ serial, transport, present, lastSeen }` |
| `plugin://changed` | P4 | `{ pluginId, phase, version? }` |

## 5. 配置键命名空间（app_settings）

- 值统一以**字符串**存储（语义在 ConfigService 校验）。
- 注册制：前端可读写的键必须先进 `ConfigService::ALLOWED_KEYS` 白名单，未注册键返回 `INTERNAL` 错误。
- 键前缀约定：`app.settings.*`（用户设置）、`app.<域>.*`（领域配置，如 `app.adb.path`）。

| 键 | 取值 | 默认 | 说明 |
|---|---|---|---|
| `app.settings.theme` | `light`/`dark`/`system` | `system` | 主题 |
| `app.settings.opacity` | 20–100 整数 | `100` | 背景不透明度（%） |
| `app.settings.log_level` | trace/debug/info/warn/error | `info` | 写入走 `log_set_level` 联动运行时重载 |

- 日志级别专用命令 `log_set_level`：除落库外还 reload EnvFilter，前端不要用 `config_set` 直接写它。
- 前端持久化策略：**SQLite 为事实源**，localStorage 仅作启动缓存（消除 FOUC）；启动时 snapshot 水合，DB 缺失的键做一次性迁移回写（兼容 P0 用户）。

## 6. 数据库

- 文件：app data 目录下 `app-reverse-tools.db`，WAL + foreign_keys ON。
- 结构变更只走 `migrations/NNN_*.sql`（版本随 `db/connection.rs` 的 `MIGRATIONS` 数组递增），SQL 自身保持幂等（`IF NOT EXISTS`）；禁止运行时隐式建表改表。
- 只存元数据与历史；大二进制（APK、日志包）进 app data 目录，库中存路径。
- 连接模型：`Db` = `Arc<Mutex<Connection>>` + `with()` 闭包；P2 出现高频写（task_logs）时评估 r2d2 池。

## 7. 新增一个命令的固定动作

1. `services/` 写业务 + 单测；
2. `commands/` 写薄命令（校验 + 转发，返回 `CoreResult<Dto>`）；
3. `lib.rs` 注册到 `generate_handler!`；需要能力授权时同步 `capabilities/default.json`；
4. `src/api/<域>.ts` 封装 + `src/types/` 加 DTO 类型；
5. 涉及事件：`core/ipc.rs` 注册事件名，前端 `api/` 出 `listen` 封装（返回 unlisten 函数）。
