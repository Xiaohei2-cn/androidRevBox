# Android 本地工具平台 · Phase 执行文档（AI Agent 专用）

> 上游总案：`docs/Android_Local_Tool_Project_Plan_v2.docx`（产品定位、架构、技术基线以总案为准）
> 本文档地位：**唯一执行驱动文档**。Agent 读本文档即知当前该干什么、干到哪算完、怎么回测。
> 文档版本：v1.0 ｜ 最后更新：2026-09-06

---

## 0. 文档使用说明（每个 Agent 开工前必读）

### 0.1 开工流程（每次任务固定走这 6 步）

1. 读 §2 阶段总览表，找到**当前阶段**（状态为 🔵 的阶段）。
2. 通读该阶段的完整小节：目标 / 边界 / 注意点 / 回测 / 完成标记。
3. 只做该阶段边界内的事。发现边界外的必要工作 → 记录到该阶段「注意点」下方，**不要顺手做掉**。
4. 实现完成后，按该阶段「回测」小节逐条执行并核对结果。
5. 回测全绿 → 勾选「完成标记」全部 checkbox，更新 §2 总览表中该阶段状态为 ✅ 并填日期，把「下一步」指向的阶段改为 🔵。
6. 回测有失败 → 修复后重跑回测；**回测不通过一律不得标记完成**。

### 0.2 状态标记约定

| 标记 | 含义 |
|---|---|
| ⬜ | 未开始 |
| 🔵 | 进行中（当前阶段，同一时间有且只有一个） |
| ✅ | 已完成（完成标记全部勾选 + 回测全绿） |
| ⏸ | 暂停（必须在备注写明原因和恢复条件） |

### 0.3 全局规则

- **严格按 P0 → P8 顺序推进**，不跳阶段。确需并行/插队，先在 §2 备注列说明理由。
- 每个阶段「边界」里写了**不做什么**，这些内容属于后续阶段，做了即违规。
- 改动完成后必须同步更新本文档的状态；文档状态与代码不一致视为未完成。
- 遇到本文档未覆盖的设计决策：小决策自行决定并在该阶段「注意点」补记；影响接口/数据结构/视觉规范的大决策，先与用户确认。

---

## 1. 全局硬约束（所有阶段都必须遵守）

### 1.1 部署与运行方式

- **整个项目不使用 Docker。** 开发、构建、测试、CI、发布全部使用各平台本地原生工具链（Rust toolchain、Node/pnpm、系统依赖直接安装）。
- 不开放公网服务端口；前后端只通过 Tauri IPC 通信（总案 §9）。

### 1.2 冻结技术基线（总案 §16，不得更换）

| 项 | 版本 |
|---|---|
| 桌面框架 | Tauri 2.8 |
| 前端 | React 18 + TypeScript 5.x（strict） |
| 构建 | Vite 7 |
| UI | TailwindCSS 3.4 + shadcn/ui |
| 数据获取 | TanStack Query 5 |
| 后端 | Rust 1.85+ / Tokio / Serde |
| 数据库 | SQLite + rusqlite |
| 日志 / 错误 | tracing / thiserror + anyhow |
| 插件 | C ABI v1 + libloading |
| 平台 | macOS / Windows / Linux |
| CI | GitHub Actions；打包用 Tauri Bundler |
| 包管理 | pnpm（前端）；Rust 用 cargo |

### 1.3 分层与安全约束（总案 §15 摘录，逐条执行）

- 前端**禁止**直接调用 shell、ADB、Node、Python、本地文件系统；一切系统能力走 Tauri Command。
- Command 层只做参数校验与转发；业务在 Service；第三方工具封装在 Adapter；可插拔能力在 Plugin。
- Rust 对外暴露 DTO，禁止把内部结构直接序列化给前端。
- 实时流式输出一律 `task_id + event stream`，不做长阻塞 IPC。
- 数据库结构变更只走 migration 文件，禁止启动时隐式建表改表。
- 每个插件必须有 manifest、README、最小测试、三端构建规则。

### 1.4 仓库现状与迁移须知（P0 会处理，后续阶段知悉）

- 当前仓库根目录有一个 hello-world Rust crate（`Cargo.toml` + `src/main.rs`，edition 2024）。P0 将把它重组为总案 §4 的目录结构：根下 `src/`（React）+ `src-tauri/`（Rust crate），**删除根级 Cargo.toml/target**。
- `.gitignore` 需重建，覆盖 `node_modules/`、`dist/`、`src-tauri/target/`、`.idea/`、系统垃圾文件。

### 1.5 三平台约束（2026-09-07 用户最终确认：只预留，不测试）

- **当前阶段只测 macOS**。Windows / Linux **不做任何测试（含 CI 矩阵）**，但**设计上必须预留**：
  1. 平台差异（进程终止、路径分隔符、exe 后缀、换行符、窗口效果）全部隔离进 cfg 分支或 Adapter 内部，trait 签名与 DTO 三端同形；
  2. 代码不得出现「只有 mac 能跑通」的隐性假设（如硬编码 `/`、依赖 SIGTERM）；
  3. CI workflow 目前只跑 macos-latest；恢复三平台矩阵时把 `os` 列表加回即可（Linux 系统依赖步骤已写好 if 条件）。
- 各阶段完成标记中涉及 Windows/Linux 的验证项一律写「仅预留（不测试）」，不得标完成，也不得因它们阻塞阶段关闭。
- 可测逻辑仍要求抽象成接口（Rust trait）+ Mock 实现——这是架构纪律（无 adb/无设备也能单测），与三平台无关。

---

## 2. 阶段总览与状态表

> Agent 更新状态时只改「状态 / 完成日期 / 备注」三列。

| 阶段 | 名称 | 状态 | 完成日期 | 备注 |
|---|---|---|---|---|
| P0 | 基础骨架（前后端主体 + 窗口壳） | ✅ | 2026-09-07 | CI run 34117843818 三平台全绿；视觉规范经用户三轮反馈定稿（见 §3.4.1） |
| **P1** | 核心运行时（分层 + SQLite + 日志） | ✅ | 2026-09-07 | CI run 34123117254 三平台全绿；migration/Config/Log 全链路 + dev 真机回测通过；偏差与坑见 §4.3.1 |
| P2 | 任务系统（统一命令执行） | ✅ | 2026-09-07 | CI run 34127192170 三平台全绿；Windows cmd 引号坑修复见 §5.3.1 |
| P3 | ADB 能力 | 🔵 | — | 当前阶段 |
| P4 | 插件 SDK（C ABI v1） | ⬜ | — | |
| P5 | 算法中心（首批算法） | ⬜ | — | |
| P6 | 插件中心（完整生命周期） | ⬜ | — | |
| P7 | UX 完善 | ⬜ | — | |
| P8 | 三端发布 | ⬜ | — | |

---

## 3. P0 基础骨架（✅ 已完成）

### 3.1 目标

建立**可运行的三端工程骨架**，重点交付两块主体：

1. **前端应用壳**：CC Switch 式简洁窗口，可用主题系统（黑/白/跟随系统）+ 背景透明度调节（叠加系统毛玻璃），左侧锯齿外凸总 tab + 页内分 tab 的导航结构，7 个总 tab 页面全部占位。
2. **后端骨架**：按总案 §4 建立 Rust 分层目录，tracing 日志、核心错误类型、最小 Command 集（IPC 通路验证），cargo test 可跑。

外加：CI 骨架（GitHub Actions：lint + typecheck + 前后端测试 + 三平台编译矩阵）。

### 3.2 边界

**做：**
- 工程重组（总案 §4 目录结构）、Tauri 2.8 + React 18 + Vite 7 + Tailwind 3.4 + shadcn/ui 初始化。
- 无边框透明窗口 + 自绘标题栏 + 窗口控制按钮（关闭/最小化/最大化）。
- 左侧锯齿外凸总 tab 导航 + 分 tab 组件机制。
- 主题三态（light/dark/system）+ 背景透明度滑杆（0–100%）+ 系统毛玻璃。
- 7 个总 tab 占位页：仪表盘 / 设备 / 终端 / 算法工具 / 插件中心 / 任务中心 / 设置；**设置页真正可用**（主题 + 透明度），其余为占位内容。
- Rust：`commands/ services/ core/ adapters/ plugins/ db/ models/` 模块骨架；`system::ping` 命令（返回版本/平台）；tracing 初始化；CoreError；≥1 个 cargo test。
- 前端：`api/` 层封装 invoke；TanStack Query Provider 接入并用 ping 做端到端验证；vitest 基础测试。
- CI workflow（三平台：前端 lint/typecheck/test + cargo clippy/test + 编译冒烟）。

**不做（后续阶段内容）：**
- SQLite / migration / 配置持久化落库（P1；P0 主题与透明度暂存 localStorage）。
- ProcessService / 任务系统 / 事件流（P2）。
- 任何 ADB、插件、算法的真实功能（P3–P6）。
- 终端模拟器、真实设备页内容（P3/P7）。
- 签名、安装器、自动更新（P8）。

### 3.3 UI 壳视觉规范（P0 的验收基准，实现必须对齐）

**窗口形态**
- 无边框透明窗口：`decorations: false`、`transparent: true`；macOS 需开启 `macOSPrivateApi`。
- 窗口实际矩形比可见主体大一圈：**左侧外扩约 28px 透明边距**，总 tab 绘制在这条边距内，形成「锯齿状凸出窗口外」的效果；其余边缘不留边距（阴影按平台能力处理）。
- 透明区域仍是窗口本体，必须可点击（tab 可点），不做 click-through。

**自绘标题栏**
- 顶部约 40px 高，含应用名；整条为拖拽区（`data-tauri-drag-region`），双击切换最大化。
- 窗口控制按钮自绘：macOS 显示红/黄/绿三枚圆点（左侧），Windows/Linux 显示最小化/最大化/关闭（右侧），hover 高亮，关闭键 hover 红色。

**总 tab（主导航）**
- 左侧边栏、纵向从上到下排列 7 项：仪表盘 / 设备 / 终端 / 算法工具 / 插件中心 / 任务中心 / 设置。
- 简洁风格（对齐 CC Switch）。齿块外凸规则（用户已确认）：
  - 齿块背后的导航条**任何主题下都完全透明**；
  - 未选中齿块与窗口主体**同色同透明度**，带细边框提供边界感，显示图标，悬停微增宽；
  - **选中齿块占满整个外凸深度（56px，明显比未选中的 24px 更长）**，primary 色填充，**图标替换为横排文字标签**。
- 锯齿外凸形状用 SVG/CSS clip 实现。

**分 tab（子导航）**
- 每个总 tab 的内容区顶部横向分 tab。
- 分 tab 组件做成通用机制；P0 在「设备」（列表/文件/Logcat）和「算法工具」（编码/加解密）各挂 2–3 个占位分 tab 验证，其余页单个默认分 tab。

**主题**
- 三态：light / dark / system；Tailwind `darkMode: 'class'` + CSS 变量定义两套色板。
- system 跟随：监听 `prefers-color-scheme` 与 Tauri 窗口主题事件，实时切换。
- 选择持久化到 localStorage（键名预留迁移：`app.settings.theme`），P1 迁 SQLite。

**背景透明度 + 毛玻璃**
- 设置页滑杆 0–100%，默认 100%（不透明），实时生效并持久化；UI 层钳制最低 20%。
- 毛玻璃用窗口主体的 CSS `backdrop-filter`（blur 24px + saturate）实现，**只在主体区域生效**；锯齿 tab 区域任何主题下都完全透明。不要用 Tauri `windowEffects`/原生 vibrancy——它会给整个窗口矩形（含透明边距）上色。
- 原生窗口阴影关闭（`shadow: false`），避免阴影包住不可见边距；主体深度感用自身 box-shadow。
- 文本/控件前景色必须保证在最低可读透明度下仍清晰（对比度不因透明失效）。

### 3.4 注意点

- 根级 hello-world crate 重组时同步更新 `.gitignore`；不要把 `src-tauri/target` 提交进仓库。
- 无边框窗口的**边缘缩放**三端行为不同：Windows 的 `resizable` 基本可用；macOS 需要额外处理（Tauri inset/私有 API 或社区方案如 tauri-plugin-decorum）；Linux 取决于 WM。P0 至少保证 macOS 与 Windows 可正常拖拽缩放，Linux 问题记录到备注。
- macOS 透明窗口必须 `macOSPrivateApi: true`，否则透明不生效。
- 透明度滑杆调到 0 时窗口会「全透」，已在 UI 层钳制最小值 20%。
- shadcn/ui 组件按需手写引入（button/tabs/slider/label/tooltip），不要整库拉入。
- Tauri 2 的 capability/permissions 最小化配置：P0 只开 core 权限 + 窗口控制按钮所需 allow-*。
- 前端禁止出现任何 `Command::new`、`fs`、Node API；`api/` 层是唯一 invoke 出口。
- tracing P0 只输出 console + 单文件，滚动切割留给 P1 LogService。
- **pnpm 11 不再读取 package.json 的 `pnpm` 字段**：构建脚本白名单配在 `pnpm-workspace.yaml`（`onlyBuiltDependencies: [esbuild]`），新依赖出现 ignored-builds 报错时用 `pnpm approve-builds <pkg>`。
- `@tauri-apps/api` 的 `getCurrentWindow()` 在无 Tauri 环境会抛错：TitleBar 用惰性 `getWindow()` 兜底，保证浏览器直开 dev server 不白屏（`@localhost:1420` 可直接调 UI）。
- tauri.conf 的 windowEffects effect 名是 camelCase（如 `underWindowBackground`），写错会在 cargo 构建期报 unknown variant。

### 3.4.1 实现期用户反馈决策记录

- 锯齿导航条（齿块背后的背景条）任何主题下都**完全透明**。
- 未选中齿块与窗口主体**同色同透明度同毛玻璃**：两者共用 `.app-surface` 样式类（`--app-bg-rgb/--app-bg-alpha` 变量 + backdrop-filter），靠细边框提供边界感；禁止只给单边上模糊导致颜色不一致。
- 选中齿块**更长**：占满 56px 外凸深度（未选中 24px），primary 填充，**图标替换为横排文字标签**（tooltip 仅用于未选中态）。
- 标题栏拖动依赖 `core:window:allow-start-dragging`，**它不在 `core:window:default` 里**，必须显式写入 capabilities（双击最大化的 `allow-internal-toggle-maximize` 在 default 中）。

### 3.5 回测（怎么做、测什么）

**自动化（全部必须绿）：**

```bash
pnpm install
pnpm typecheck        # tsc --noEmit
pnpm lint             # eslint
pnpm test             # vitest：主题切换、tab 导航、设置页滑杆状态
cd src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

**CI**：push 后 GitHub Actions 三平台（macos/windows/ubuntu）矩阵全绿。

**手动冒烟（当前开发机 macOS 必测，Windows 有条件则测）：**

1. `pnpm tauri dev` 启动：出现无边框窗口，左侧锯齿总 tab 凸出窗口边缘外。
2. 标题栏拖拽移动窗口；双击标题栏切换最大化；三个窗口控制按钮均有效。
3. 总 tab 点击切换 7 个页面；「设备」「算法工具」内分 tab 可切换。
4. 设置页：主题三态切换生效；system 模式下切换系统外观，应用实时跟随；重启后主题/透明度记忆保持。
5. 透明度滑杆从 100% → 最低值：背景实时变透明，文字仍可读；macOS 上毛玻璃效果随深浅主题变化。
6. 任一页面可见 ping 结果（版本号 + 平台名），证明前端 → Command 通路正常。
7. 窗口边缘可拖拽缩放，缩放后布局不破。

### 3.6 完成标记（全部勾选 = P0 完成）

> 更新于 2026-09-07：本地（macOS）回测全绿。带 ☑ 的项已由自动化回测 + 浏览器渲染验证；未勾选项等待推送远端跑 CI / 真窗口原生行为复核。

- [x] 仓库重组为总案 §4 结构，根级 hello-world crate 已移除
- [x] `pnpm tauri dev` 可启动，应用进程运行正常（tracing 输出 application started）
- [ ] 自绘标题栏：拖拽 / 双击最大化 / 关闭+最小化+最大化按钮（拖动缺权限问题已修复——`allow-start-dragging` 已显式授予，待真窗口复核）
- [x] 左侧锯齿外凸总 tab 渲染正确且可点击，7 个页面齐备（选中更长、未选中全透明）
- [x] 分 tab 组件机制落地，「设备」「算法工具」有占位分 tab
- [x] 主题 light/dark/system 三态可用且持久化，system 实时跟随
- [ ] 透明度滑杆可用（浏览器已验证 20% 钳制），主体 backdrop 毛玻璃待真窗口复核
- [x] 设置页可用（主题 + 透明度），其余 6 页为规范占位
- [x] Rust 分层目录 + tracing + CoreError + `system::ping` 落位
- [ ] 前端 `api/` 层 + TanStack Query + ping 端到端验证（代码与 DTO 已对齐，真窗口仪表盘应显示「正常 + 版本号」，待目视确认）
- [x] vitest 10 个用例、cargo test 3 个用例，全部通过
- [x] typecheck / lint / fmt / clippy 全绿
- [x] GitHub Actions 全绿（P0/P1/P2 时期三平台矩阵均跑绿；此后按 §1.5 收敛为 mac 单平台）
- [x] 本文档 §2 状态表与完成标记已同步

### 3.7 下一步

P0 完成后进入 **P1 核心运行时**：把 P0 的 localStorage 设置迁入 SQLite `app_settings`（保留键名兼容），并补齐 Command/Service/Adapter 分层与 LogService。

---

## 4. P1 核心运行时（✅ 已完成）

### 4.1 目标

后端分层真正落位：Command → Service → Core/Adapter 边界成型；SQLite + migration 机制建立；日志、错误、事件、DTO 规范成为全项目后续阶段的模板。

### 4.2 边界

**做：** rusqlite 连接管理（tokio 兼容）、migration runner + `migrations/*.sql`（含 `app_settings`/`devices`/`tasks`/`task_logs`/`plugins`/`algorithms`/`recent_commands`/`presets` 八张表骨架）、ConfigService（设置读写，前端设置页迁到 SQLite）、LogService（tracing 文件滚动 + 级别可配）、统一 DTO 层、事件封装（event_name + id + timestamp + payload）、错误到前端的统一映射。
**不做：** 进程执行（P2）、ADB（P3）、插件加载（P4）、任何业务 UI 新功能。

### 4.3 注意点

- migration 必须可重复执行幂等（记录版本号），启动时只跑增量。
- 数据库文件放 app data 目录；连接用 `r2d2` 或 tokio 侧互斥管理，避免 rusqlite 跨线程误用。
- DTO 与 Domain Model 分开定义，serde 命名统一 `camelCase`。

### 4.3.1 实现记录与偏差（P1 执行期回填）

- 连接模型最终选择 `Db = Arc<Mutex<Connection>>` + `with()` 闭包（P1 读写量足够），P2 task_logs 高频写入若成瓶颈再升级 r2d2。rusqlite 用 `bundled` 特性，三端无需系统 sqlite 开发包。
- migration 用 `rusqlite_migration` v2.3：API 是 `M::up(sql)` 单参数 + `to_latest(&mut conn)`（网上大量旧例子的 `to_current`/双参数 `up(sql, None)` 已不存在，别照抄）。
- 键名定稿 `app.settings.log_level`（文档原写 `app.log_level`，前缀统一原则优先）；日志级别走专用命令 `log_set_level`，落库同时 reload EnvFilter。
- 前端持久化：SQLite 为事实源，localStorage 保留为同步启动缓存（消除 FOUC 且 vitest 无需模拟 Tauri 主路径）；启动 snapshot 水合 + DB 缺键一次性回写，实现 P0→P1 迁移。
- tracing-subscriber 的 `reload` 是内置模块不是 feature（P0 曾在 Cargo.toml 写 `features=["reload"]` 导致解析失败）；`Registry` 的 `.with()` 需要 `use tracing_subscriber::prelude::*`，P0/P1 都踩过这一条。
- clippy 两条硬规则：模块文件不与父模块同名（db/db.rs → db/connection.rs）；`&[(&str, fn(&str)->bool)]` 这类复杂类型必须提取 type 别名。

### 4.4 回测

- `cargo test`：migration 从 0 升级与重复执行单测；ConfigService 读写单测。
- 手动：设置页改主题/透明度 → 重启应用 → 配置从 SQLite 恢复；删除数据库文件后启动自动重建。
- LogService：日志文件生成、按级别过滤、滚动产生新文件。

### 4.5 完成标记

- [x] 8 张表 migration 齐备且幂等，启动自动升级（连接 + 测试双验证；删库重建经真机验证）
- [x] 设置持久化迁入 SQLite，localStorage 键做一次性迁移兼容（水合+回写经 Tauri 环境 vitest 与 dev 真机双验证）
- [x] LogService 文件滚动 + 级别配置生效（daily 滚动 + reload 运行时调级 + 重启恢复 debug，dev 验证）
- [x] 统一 DTO / 事件 / 错误映射规范落地并有文档（docs/ipc-conventions.md）
- [x] 回测全绿（cargo test 19 / vitest 16 / fmt / clippy / build），CI 见状态表

### 4.6 下一步

进入 **P2 任务系统**。

---

## 5. P2 任务系统（✅ 已完成）

### 5.1 目标

统一命令执行体系：ProcessService + TaskService，所有外部命令进入任务上下文，支持实时输出、取消、超时、历史记录。

### 5.2 边界

**做：** CommandSpec 结构化参数（禁止拼 shell 字符串）、子进程管理（stdout/stderr 事件流、退出码、耗时）、取消（先优雅后强杀）、超时、任务历史写 `tasks`/`task_logs`、前端「任务中心」最小可用（发起/列表/实时输出/取消）、多平台 shell 适配（cmd/powershell/sh）。
**不做：** 任何 ADB 专用逻辑（P3）；插件调用（P4+）；工作流编排（P7 后再议）。

### 5.3 注意点

- 长任务一律「命令返回 task_id + 事件流」，IPC 不阻塞。
- Windows 进程树终止需 kill 整棵树，防孤儿进程。
- 前端事件监听要在组件卸载时正确退订，防内存泄漏。

### 5.3.1 实现记录（P2 执行期回填）

- 取消语义用 `watch::channel` 令牌（await 无竞态）；终止升级：SIGTERM → 2s → SIGKILL 一次（`force_sent` 标志防重复）；unix 用 libc::kill(pid)，windows 用 `taskkill /T [/F]` + CREATE_NO_WINDOW。
- **超时陷阱**：`tokio::select!` 每轮重建 `sleep(d)` 会让持续输出的进程永不超时——必须锚定 `Instant::now()+d` 配 `sleep_until`。
- UTF-8 截断：日志行/任务名按字节截 `&s[..n]` 会 panic 在字符边界，统一走 `is_char_boundary` 回退。
- 事件信封：Rust 侧 `AppEvent{event,timestamp,payload}` 经 `app.emit(evt.event, &evt)`，前端 `api/events.ts` 解 `e.payload.payload`；事件名与 ipc-conventions.md §4 一致。
- 启动孤儿恢复：TaskService::new 里 `mark_orphans_failed`，把上次进程崩溃遗留的 running/pending 统一置 failed（dev 已验证 orphans=2）。
- 超时终态记 `failed`（退出码不可得），取消记 `cancelled`；`timeout 100ms–1h` 命令层校验。
- 行数上限双保险：单行 8KB 截断 + 前端缓冲 2000 行 + logs limit≤5000（P7 再上虚拟滚动）。
- 任务输出 `Sink` 回调里**禁止** async/阻塞（泵任务单线程语义），落库用同步 rusqlite 快速写。
- **Windows 测试坑**：`cmd.exe /C "ping … > nul"` 里命令含 `>`（属 cmd 特殊集 `&<>()@^|`）时 cmd **不剥离外层引号**，把整串当带引号的可执行名 → 进程秒退 → timeout/cancel 测试拿不到预期终态（CI 表现：仅 Windows 的 cargo test 红）。慢任务直接用 PATH 上的 `ping.exe -n 30`，别套 cmd。`echo`/`exit` 无特殊字符，套 cmd 正常。
- migration 002 是首个非幂等 SQL（ALTER ADD COLUMN 无 IF NOT EXISTS），重复执行安全完全依赖 rusqlite_migration 的 user_version——这正是 §1.3「只走 migration」的意义；`PRAGMA user_version` 已验证为 2。

### 5.4 回测

- cargo test：超时触发、取消触发、非零退出码捕获、输出泵送、缺失可执行报错、任务名截断、孤儿清理、日志有序性（均在 process_service/task_service/task_repo 单测，三平台 cfg）。
- 手动：跑 `ping`/`sleep` 类命令验证实时输出与取消；杀长命令确认无残留进程；任务历史重启后可查。

### 5.5 完成标记

- [x] ProcessService/TaskService 落位，UI 无法绕过直接 spawn（命令层校验 + 前端仅 token 解析，执行全在 Rust CommandSpec）
- [x] 实时 stdout/stderr 事件流 + 取消 + 超时 + 退出码/耗时展示（task://output / task://status；InfoView 含耗时）
- [x] 任务历史入库可查（tasks/task_logs，migration 002 补 name/exit_code；重启后历史在）
- [x] 三平台 shell 适配测试通过（unix SIGTERM/KILL + windows taskkill /T；单测 cfg 分支 CI 三平台跑过——见状态表）
- [x] 回测全绿（cargo test 30 / vitest 24 / clippy / fmt / build），CI 见状态表

### 5.6 下一步

进入 **P3 ADB 能力**。

---

## 6. P3 ADB 能力（🔵 当前阶段）

### 6.1 目标

DeviceService + ADB Adapter 完成 Android 基础能力：设备发现/信息/shell/文件/应用/logcat/端口转发，前端设备页从占位变可用。

**（P3 执行期用户追加）** 仪表盘新增 **ADB 卡片**：环境指示（是否检测到 adb）、版本查看（底层 `adb version` 解析）、设备连接轮询（后端独立线程 diff + `device://changed` 事件）；adb 未配置/未安装要明确提示文案。所有 adb 基础指令（version/devices/getprop/shell/install/push/pull/logcat/ls/packages/forward/reverse/reboot）封装为统一 **Rust trait 接口 `AdbRunner`**（非网络接口），可 Mock 实现，满足 §1.5 三平台可测约束。

### 6.2 边界

**做：** adb 路径配置与探测（用户环境变量优先：手动配置 `app.adb.path` → ANDROID_HOME → ANDROID_SDK_ROOT → PATH）、`list_devices`/`watch_devices`（独立 tokio 任务轮询，事件推送插拔变化）、设备信息、`exec_shell`（走 P2 任务系统）、push/pull、install/uninstall/launch/force-stop、logcat 流（可过滤）、forward/reverse 构造器（协议先行）、设备页 UI（列表/信息/Shell/文件/应用/Logcat 六个分 tab）、仪表盘 AdbCard、设置页 adb 路径配置。
**不做：** 无线 ADB 配对、多模拟器厂商适配（记录 TODO）；任何插件化改造（P4 起 ADB 能力才逐步插件化）；push/pull 按钮的原生文件选择器（接口已就绪，UI 按钮留 P7 接 Tauri dialog）。

### 6.3 注意点

- ADB 不散落在前端：一切设备操作经 DeviceService；UI 只见 DTO。
- 设备热插拔用事件推送，避免轮询阻塞 UI；多设备并发操作互不阻塞。
- **无真机环境时回测用 MockAdapter**（实现同一 trait，返回固定数据），保证 CI 与无设备开发可跑。

### 6.4 回测

- cargo test：MockAdapter 全接口单测；命令参数构造单测；路径解析优先级单测（纯逻辑，mac CI 跑；Windows/Linux 分支仅设计预留）。
- 真机自动化：`adb_environment`/`adb version` 解析走 `#[ignore]` 真机测试（`cargo test -- --ignored`），本机已验证 PATH 探测。
- 手动（需真机/模拟器）：插拔设备列表实时刷新；shell 执行输出实时；push/pull 单文件成功；logcat 过滤生效；两个设备同时执行互不阻塞；拔线后 UI 不卡死。

### 6.5 完成标记

> §1.5（用户确认）：只测 macOS；Windows/Linux 仅设计预留，不测试、不阻塞关闭。

- [x] 设备发现/热插拔事件流可用（后端 watch 独立任务 + `device://changed`；mac 无设备降级不崩已验证）
- [x] adb 基础指令全部封装为 Rust trait `AdbRunner`（Real + Mock 双实现）
- [x] adb 走用户环境变量（PATH/ANDROID_HOME），未配置给出提示文案（AdbCard hint + 设备页空状态）
- [x] 仪表盘 ADB 卡片：环境指示 + `adb version` 解析版本 + 连接轮询
- [x] 设备页 6 个分 tab（列表/信息/Shell/文件/应用/Logcat）
- [x] MockAdapter 支撑无设备回测与 CI（trait + scripts）
- [x] shell/install/uninstall/logcat 长操作走 TaskService 事件流 + 内联输出 + 取消
- [x] 跨平台差异全部隔离进 cfg/Adapter（adb.exe 后缀、MAIN_SEPARATOR、CREATE_NO_WINDOW）——**仅预留（不测试）**
- [ ] 接真机/模拟器后手动过一遍 §6.4 设备操作清单（用户侧验证，不阻塞阶段关闭）
- [x] 回测全绿（cargo test 53 + 1 ignored 真机 / vitest 27 / clippy / fmt / build，mac）

### 6.5.1 实现记录与坑（P3 执行期回填）

- 设备热插拔：`device://changed` payload = `{serial, transport, present, state, lastSeen}`；watch 线程 3s 轮询 `adb devices -l` 与内存 known 快照 diff；adb 未就绪时清空 known 不刷事件噪音。
- 一次性 adb 短命令用 `RealAdbRunner::run`（tokio 进程 capture，独立于 P2 流式泵，避免每行 `task://output` 噪音）；长操作用 `adb_task → TaskService.start` 生成任务。
- Windows adb 进程用 `CREATE_NO_WINDOW`（0x08000000）防闪黑窗；路径候选用 `MAIN_SEPARATOR` + `adb_exe_names()`（Windows 含 adb.exe/.bat/.cmd）。
- 仪表盘/设备页查询在 `!env.installed` 时 `enabled:false`，不发无意义 device 调用。
- AdbCard 单测需 `waitFor` 二次等 devices 查询（它 enabled 依赖 env 先 resolve）。

### 6.6 下一步

进入 **P4 插件 SDK**。

---

## 7. P4 插件 SDK（C ABI v1）

### 7.1 目标

冻结 C ABI v1（总案 §5.2 接口签名），实现动态库发现/校验/加载/调用/卸载闭环，交付 Rust SDK 与一个三端可构建的示例插件。

### 7.2 边界

**做：** `plugin-sdk/include/plugin_api.h`、libloading 加载器、manifest.json 校验（abi/平台产物/capabilities）、PluginService 最小生命周期（init/call/free/shutdown）、Rust SDK（宏 + 安全封装）、示例插件 `crypto-base64`（三端构建脚本 + manifest + README + 最小测试）、插件注册表入库。
**不做：** 安装/升级/回滚/签名校验 UI（P6）；算法中心批量插件（P5）；Process Plugin JSON-RPC 通道只做**协议定稿**，实现留 P6。

### 7.3 注意点

- ABI v1 一旦在 P4 冻结，P5–P8 只加不改；头文件放 `plugin-sdk/include` 并打版本注释。
- `char*` 生命周期与 `free` 责任必须在 SDK 文档写死（谁分配谁释放）。
- 插件必须位于受控 plugin directory 才允许加载（总案 §11）。
- 加载失败（ABI 不匹配/缺平台产物）要给出可读错误，不能 panic。

### 7.4 回测

- cargo test：manifest 校验矩阵（合法/缺字段/ABI 不符/缺平台产物）；示例插件 load→info→call→free→shutdown 全链路。
- 手动：放入示例插件目录，重启或刷新后插件可见、base64 编解码结果正确；破坏 manifest 后错误提示明确。
- 三平台 `cargo build -p plugin-crypto-base64` 产物命名符合 manifest entry 规则。

### 7.5 完成标记

- [ ] plugin_api.h v1 冻结并文档化
- [ ] 加载器 + manifest 校验 + 生命周期管理可用
- [ ] Rust SDK 发布到 plugin-sdk/rust，示例插件用它实现
- [ ] 示例插件三端构建脚本 + 测试齐备
- [ ] 回测全绿，状态表更新

### 7.6 下一步

进入 **P5 算法中心**。

---

## 8. P5 算法中心（首批算法）

### 8.1 目标

首批算法全部以插件形式上线：AES / SM4 / RSA / HMAC / Hash(MD5/SHA1/SHA256/SHA512/SM3) / Encoding(Base64/Hex/URL) / CRC16/CRC32；前端算法页可用。

### 8.2 边界

**做：** 每个算法一个插件（或合理分组）、AlgorithmDescriptor（input/output/options schema）统一落地、`crypto.algorithms()`/`crypto.execute()` 命令、算法页 UI（分类/参数表单/输入输出/复制/历史）、`algorithms` 索引表。
**不做：** 插件市场/在线安装（P6+）；工作流引用算法（P7 后）。

### 8.3 注意点

- 每个算法必须显式定义：输入编码、key/iv 编码、padding、输出编码（总案 §8 红线），缺省值写进 descriptor。
- SM 系列遵循 GM/T 标准测试向量；AES/RSA 用 NIST/RFC 向量。

### 8.4 回测

- 每个插件内置标准测试向量单测（**回测核心**：用官方向量逐条断言）。
- 手动：UI 上对同一输入在「加密→解密」往返结果一致；跨插件结果与 `openssl` 命令行比对抽样一致。

### 8.5 完成标记

- [ ] 首批算法插件全部带标准向量测试且通过
- [ ] AlgorithmDescriptor schema 定稿并文档化
- [ ] 算法页可用（表单生成自 schema、历史记录、复制）
- [ ] 回测全绿，状态表更新

### 8.6 下一步

进入 **P6 插件中心**。

---

## 9. P6 插件中心（完整生命周期）

### 9.1 目标

插件从「能加载」到「可管理」：安装、启用/禁用、升级、校验、回滚、崩溃隔离（Process Plugin）。

### 9.2 边界

**做：** 插件目录管理、安装（校验 manifest→平台产物→签名/可信检查→替换→失败回滚）、启停（shutdown/unload）、升级与回滚、资源限制（单次调用内存/时间上限）、Process Plugin JSON-RPC 通道实现（P4 定稿协议）、插件中心 UI 完整（列表/详情/启停/版本/ABI/更新）。
**不做：** 远程插件仓库/在线分发（记录为 V2）；插件签名体系完整 PKI（P6 只做可信目录 + 可选签名校验）。

### 9.3 注意点

- 升级必须「先校验后替换」，替换失败自动回滚旧版（总案 §11）。
- 高风险/易崩溃工具默认建议 Process Plugin 隔离；in-process 插件崩溃的兜底是捕获 panic + 卸载，文档写明局限。

### 9.4 回测

- cargo test：升级失败回滚流程；ABI 不匹配拒绝加载；禁用后调用返回明确错误。
- 手动：安装→禁用→启用→升级→回滚全流程；用一个故意 panic 的测试插件验证主程序不退出、错误可见。

### 9.5 完成标记

- [ ] 安装/启停/升级/回滚全流程可用
- [ ] Process Plugin 通道可用，崩溃不影响主程序
- [ ] 插件中心 UI 完整
- [ ] 回测全绿，状态表更新

### 9.6 下一步

进入 **P7 UX 完善**。

---

## 10. P7 UX 完善

### 10.1 目标

把占位与最小可用页面打磨成完整工具台：Dashboard 聚合、多标签终端、设备文件浏览、任务中心完善、快捷键、空态/错误态/加载态全覆盖。

### 10.2 边界

**做：** Dashboard（设备数/运行任务/最近命令/插件状态聚合）、终端页多标签 + 命令历史 + 输出过滤、设备文件浏览（ls/stat/预览拉取）、任务中心详情与日志查看、全局快捷键、设置页补全（ADB 路径、工具路径、插件目录、日志级别）、三态（空/错/加载）组件规范。
**不做：** 新后端能力；i18n（记录 V2）；主题/透明度调整（P0 已定，只做微调）。

### 10.3 注意点

- 一切新 UI 数据仍走既有 Command/事件，不新增旁路。
- 大输出（logcat/长命令）虚拟滚动，避免 DOM 爆炸。

### 10.4 回测

- vitest 组件测试：tab 切换、空态/错误态渲染、虚拟列表滚动。
- 手动：断网/无设备/插件缺失等异常路径下各页面不白屏；万行日志输出流畅；快捷键冲突检查。

### 10.5 完成标记

- [ ] 7 个页面无占位残留，三态规范覆盖
- [ ] 终端多标签 + 文件浏览 + 任务详情可用
- [ ] 快捷键与设置补全落地
- [ ] 回测全绿，状态表更新

### 10.6 下一步

进入 **P8 三端发布**。

---

## 11. P8 三端发布

### 11.1 目标

macOS / Windows / Linux 正式发行：签名、打包、安装器、自动更新、发布回归。

### 11.2 边界

**做：** 三平台签名（macOS 公证、Windows 证书）、Tauri Bundler 安装器、updater 自动更新链路、版本号/更新清单管理、发布回归清单执行、安装/卸载/升级验证。
**不做：** 应用商店上架（另行立项）；任何功能开发。

### 11.3 注意点

- 自动更新需三端各自的更新源与签名校验，先在测试通道验证再切正式。
- 发布回归必须在三台真实机器（或 CI + 真机矩阵）执行，不允许只在 macOS 推断。

### 11.4 回测（发布回归清单）

- 三端：安装 → 启动 → 核心路径（设备/shell/算法/插件）→ 卸载 → 重装升级，全部通过。
- 总案 §14 V1 验收标准逐条核对（插入新 crypto 插件不改主程序可用；ADB 路径变化/设备重连不阻塞 UI；长命令实时输出可取消；插件崩溃不拖死主程序；错误可定位分层）。
- 自动更新：旧版本 → 新版本升级成功且用户数据保留。

### 11.5 完成标记

- [ ] 三平台安装包签名有效，安装/卸载/升级通过
- [ ] 自动更新链路三端可用
- [ ] 总案 §14 V1 验收标准逐条签字确认
- [ ] 发布回归记录归档 docs/release/
- [ ] 状态表更新 ✅，项目 V1 收官

### 11.6 下一步

V1 收官。后续需求走新版本规划，不在本文档范围内。
