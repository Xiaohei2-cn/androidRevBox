# 接入备忘：给这套工具加一个 Zygisk 能力

> 写给"下次要加接口的人（包括未来的自己）"。这里的每一条都对应仓库里真实存在的代码，
> 不是设想。定位不到就当作文档 bug，改它。
>
> 最近一次核对：2026-09-21（AR10.1 把下面的兼容矩阵从文档搬进了代码与测试）。

## 1. 先看这条链路有几层

```
桌面界面  src/features/**            （React，只做展示与交互）
   │  Tauri command
命令层    src-tauri/src/commands/zygisk.rs   （薄：校验参数 + 转调 Service）
   │
服务层    src-tauri/src/services/zygisk_applist.rs  （路由、能力可用性、降级说明）
   │  公共协议 agent-protocol（typed DTO，方法名如 package.list_localized）
Agent     android-agent/src/provider/zygisk.rs     （私有子协议的客户端）
   │  私有子协议：TCP 127.0.0.1:11501，H/OK/L/E/S/X 行协议 + 分帧 JSON
模块      android-zygisk/applistpro/jni/main.cpp   （companion 常驻服务）
   │  JNI
helper    android-zygisk/applistpro/helper/src/pro/applist/HelperPro.java （Framework API）
```

**两副协议不要混**：`agent-protocol` 是 Desktop↔Agent 的公共语义；Agent↔模块的私有子协议
DTO 留在 `android-agent`/模块两侧，**不要**把它塞进 `agent-protocol` 让界面看见。
（这条是 AR10 的硬边界：模块内部格式可以随版本改，公共契约不能跟着抖。）

## 2. 四条红线

1. `android-zygisk/applist/` 是学习用的原始 demo，**一个字节都不改**。要改行为就在
   `applistpro/` 里加，两个模块模块 id、端口、产物都不同，可以并存。
2. 不动态加载来源不明的 `.so`/脚本/dex。模块只加载自己目录里的 `helper.dex`。
3. 令牌不进日志、不进错误信息。`serve_session` 里对 `H ` 行做了截断（`line[4]='\0'`），
   Agent 侧同样只回显模块返回的状态行——改这块代码时别把它打印出来。
4. 特权只走"写死的固定脚本 + 校验过的参数"（D038）。想加新的特权动作，
   就在 `android-agent/src/provider/privileged.rs` 加模板 + typed 方法 + 单测 + 真机腿，
   不要开一个"能传命令"的 API。

## 3. 私有子协议现状（v2，端口 11501）

| 方向 | 行 | 说明 |
| --- | --- | --- |
| Agent→模块 | `H <proto> <token>` | 握手。`proto` 必须等于模块的 `PROTOCOL_V2` |
| 模块→Agent | `OK <proto> <version> <versionCode> <deviceLocale> <caps...>` | `caps` 是空格分隔的能力位 |
| Agent→模块 | `S` | 状态查询，返回同 `OK` 形状（不需要能力位） |
| Agent→模块 | `L <locale\|-> <all\|user\|system> <0\|1>` | 需要能力 `list` |
| Agent→模块 | `E <package>` | 需要能力 `export`，返回分帧文件列表 |
| Agent→模块 | `P <package>` | 需要能力 `describe`；单包元数据（显示名/版本/分包集合），两帧：条目 + `{"final":true,"count":1,"splitCount":N}`。包不存在回 `{"notFound":"<pkg>"}` + `count:0`（**不是** ERR，参数错不该把方法推进熔断） |
| Agent→模块 | `X` | 结束会话 |
| 任意响应 | `ERR <code>[ <msg>]` | 无消息时带尾随空格，所以按空白切分而不是整串比较 |
| 数据帧 | 4 字节长度头 + JSON | 末帧含 `"final": true` |

能力位常量：`CAP_LIST="list"`、`CAP_MANIFEST="manifest"`、`CAP_EXPORT="export"`、
`CAP_DESCRIBE="describe"`、`CAP_HANDLERS="handlers"`
（`android-agent/src/provider/zygisk.rs`，与模块 `OK` 行里那几个词**必须逐字相同**）。
能力列表由 `HANDLERS[]` 单点生成（AR10.2 起），加命令只需要改注册表那一行。

超时与上限（改数值前先想清楚它挡住哪种事故）：

| 位置 | 值 | 挡住什么 |
| --- | --- | --- |
| Agent `CONNECT_TIMEOUT` | 800 ms | 端口没人听时快速失败，不拖 UI |
| Agent `LINE_TIMEOUT` | 60 s | 单行应答的上限 |
| Agent `CHUNK_TIMEOUT` | 120 s | 导出大文件时的分帧读 |
| Agent `MAX_LINE_BYTES` | 6 MiB | 模块内部 4 MiB 缓冲 + 余量；防无限流 |
| Agent `ROOT_TIMEOUT` | 2.5 s | root 探测（授权弹窗没点时不能挂死会话） |
| 模块 `IDLE_TIMEOUT_MS` | 30 s | 半开连接自动收尾 |
| 模块 `MAX_FRAME_LINES` | 200000 | 单次响应行数上限 |

## 4. 兼容矩阵（AR10.1 定案，四条都是代码实际行为）

1. **主版本号严格匹配**。不猜"差不多兼容"。线格式真变了才升版本。
2. **能力位是唯一的增量通道**。模块没宣告某能力 → 对应方法在**发出命令之前**就失败：
   `unsupported_method` + `details.reason = "capability_missing"`，并带上缺哪个能力、
   模块版本、它实际宣告了哪些能力。不是让模块回一个莫名其妙的 `unknown_command`，
   更不是等超时。
3. **旧 Agent + 新模块必须能用**：Agent 只按自己认识的能力名取值，陌生的原样忽略；
   所以新增能力对旧端是纯加法。
4. **v2 不可用 ≠ Zygisk 不可用**：按 v2 自有模块 → v1 demo → 不可用 的顺序探测，
   `zygisk.status` 如实说明现在在哪一档、少了什么（AR5.3 契约）。
   典型现场：装了 v2 但撤销了 Shell 授权 → 端口在听、读不到令牌 → 退回 v1，
   并明说"指定 locale 与 labelSource 不可用"（这条有真机腿固定：
   `real_agent_zygisk_v2_locked_falls_back_to_v1_with_reason`）。

新模块 + 旧 Agent 也必须在：老 Agent 不认识的能力名被忽略即可（测试
`v2_capability_gate_follows_the_compat_matrix` 覆盖这一向）。

## 5. 加一个新接口的落地清单

假设要加"读某个包的 `AndroidManifest` 组件摘要"，能力位取 `manifest`（已存在）。
按顺序改，六步，缺一步就会出现"能编过但界面看不懂"：

1. **模块 C++（`applistpro/jni/main.cpp`）**
   - 在 `serve_session` 的命令分发里加分支（照 `L ` 的写法：先 `sscanf` 解析，
     再逐字段白名单校验，非法就 `send_err(cfd, "bad_request", "<用法>")`）。
   - 参数一律 `sscanf("%Ns")` 带长度上限，禁止无界拷贝；`char line[512]` 是命令行上限。
   - 结果用分帧写出，最后一帧带 `"final": true`；行数超限要发 `ERR too_many_items`，
     不要静默截断（截断必须让上层知道）。
   - 需要 Framework API 时通过 helper 走（见第 6 步），不要在 native 里反射私有 API。
   - 如果这是**新能力**：只往 `HANDLERS[]` 加一行（命令词、能力名、target、
     permission、该命令自己的超时与响应上限、能否安全中断）。`OK`/`STATUS`
     行的能力列表与注册表自描述都由这张表生成，**没有第二处可漏**（AR10.2 之前
     是两处手写，加命令必漏一处；漏掉的那条对 Agent 就等于"模块没这能力"）。
   - 只读命令的失败口径：参数/目标不合法 → `bad_*` 或结构化 `{"notFound":...}`，
     **不计入熔断**；helper 超时/崩溃/超限才 `note_internal_failure`。
2. **helper（`helper/src/pro/applist/HelperPro.java`）**
   - 只加静态入口，返回可 JSON 化的扁平结构；保持"冷启动约 0.4 s"的意识：
     一次查询一次进程，能合并就合并，别在循环里反复起。
3. **Agent 侧能力常量与门控（`android-agent/src/provider/zygisk.rs`）**
   - 新能力加 `pub const CAP_XXX: &str = "xxx";`
   - 发命令处调用 `pro_command_frames_with_token(token, request, CAP_XXX, METHOD_NAME)`
     或在自带握手循环里调用 `ensure_capability(&hello, CAP_XXX, METHOD_NAME)?`。
     **这一步就是"旧模块没有该能力时不超时"** 的来处，别省。
4. **公共协议（`agent-protocol/src/methods.rs`）**
   - 加方法名常量 + typed Params/Result（`#[serde(default, skip_serializing_if)]`
     给可选字段，别让旧报文反序列化失败）。
   - 注意：这些 DTO 会**直接跨 IPC 到界面**，字段线格式是 snake_case。
     前端类型必须照着写，`src-tauri/tests/ipc_dto_wire_shape.rs` 会核对（D043 定案：
     暂不加 camelCase 翻译层）。
5. **Agent Provider 注册**
   - 加进 `ZYGISK_METHODS`，在 `handle` 的 match 里接上，capability 的
     `available/unavailable_reason` 要按"模块档位"给（AR5.3 的六条要求）。
6. **Desktop 侧**
   - `services/zygisk_applist.rs` 加方法（走 Agent，**不给 Legacy 回退腿**——
     Zygisk 专属能力 ADB 抄不出来，回退只会把"缺模块"伪装成"抄的功能"）；
   - `commands/zygisk.rs` 加 command + `lib.rs` 注册；
   - 前端 `src/api/zygisk.ts` + 页面。文案要五语齐全，
     `pnpm i18n:check` 会要求 5 个语言目录键数一致（多一个少一个都红）。

### 验证顺序（别跳）

```bash
# ① 模块侧协议自检（改协议先看它红不红）
python3 android-zygisk/applistpro/tools/v2_check.py        # 需 adb + 端口转发

# ② 单元/契约
cargo test -p android-agent
cargo test -p app-reverse-tools --test agent_host_e2e      # provider/capability 基线
cargo test -p app-reverse-tools --test ipc_dto_wire_shape  # DTO 键名两侧对齐
pnpm typecheck && pnpm test

# ③ 重新编译模块 + 装机（要重启）
bash android-zygisk/applistpro/build.sh
adb push android-zygisk/applistpro/applistpro.zip /data/local/tmp/
adb shell su -c 'ksud module install /data/local/tmp/applistpro.zip' && adb reboot

# ④ 真机腿（重启后）
APPLIST_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_zygisk -- --ignored --nocapture --test-threads=1
```

重启后的经验值：**热替换 `helper.dex` 不可靠**——`su -c cp` 能写进去、模块也会加载新 dex，
但写完 su 域自己就读不回那个文件了，等于拿无法校验的路径迭代（D020）。所以走
"重装 + 重启"，别省这一次重启。

## 6. 加"第二个模块"而不是加方法时

`android-agent` 侧的探测是**变体表**驱动（`variant.module_id()` / `variant.sub_protocol()`，
端口 11500=demo v1、11501=pro v2）。新模块要：新模块 id、新端口、新子协议版本常量、
`/data/adb/modules/<id>/token` 的读取路径，以及 `zygisk.status` 里 `module_id` 的归属。
优先级规则写死为"自有模块 > demo > 不可用"，别在调用点各自排序一遍。

## 7. 改模块代码之后要做的操作（以及它不是"刷机"）

**这里没有任何刷机动作**：不改 boot 镜像、不改分区、不刷 ROM、不解 BL。做的只是
KernelSU 装模块这个常规功能 + 一次正常重启，跟你在管理器里点"安装模块"是同一件事。

```bash
bash android-zygisk/applistpro/build.sh                 # 只在电脑上编译
adb push android-zygisk/applistpro/applistpro.zip /data/local/tmp/
adb shell su -c 'ksud module install /data/local/tmp/applistpro.zip'
adb reboot                                              # 唯一需要离手的 2~3 分钟
```

为什么省不掉那次重启：模块的 C++ 库由 Zygisk 在开机时注入 zygote，`helper.dex` 由它在
进程 specialize 时加载；跑着的进程不会重读这些文件。想热替换也不是完全做不到，而是
**做不到可验证**——实测 `su -c cp` 覆盖 `helper.dex` 能写进去、模块也会加载新 dex，
但写完以后连 su 自己都对那个文件 `Permission denied`，等于往看不见的地方塞东西
（D020）。所以走"重装 + 重启"。

风险与退路：模块有 bug 最坏的常见表现是该模块功能不可用（`zygisk.status` 会报
`faulted`），内核与系统本身不受影响；真要出问题，KernelSU 管理器里禁用/删除模块再重启
即可回到原状。注意：AR10.5 的"启动安全（bootloop 防护）"我们**还没主动验证过**，
现在只有"模块异常只会让 zygisk.status 报 faulted，不影响开机"这一条观察——所以
第一次装新写的模块前，值得先确认你知道怎么在恢复模式下删模块，或者干脆等 AR10.5 验完。

## 8. 卸载 / 禁用 / 恢复（现场最常问的）

```bash
adb shell su -c 'ksud module disable applistpro'   # 退到 v1 demo（装着的话）
adb shell su -c 'ksud module enable applistpro'
adb shell su -c 'ksud module remove applistpro'    # 或 KernelSU 管理器里删
```

禁用/卸载后 Agent 必须仍然可用：`zygisk.*`、`package.list_localized`、
`package.export_apk` 明确报能力缺失，`package.list`（Shell 等价实现）照常工作。
这条不靠记性——`real_agent_zygisk_absent_reports_honest_failure` 在无模块设备上验。

## 9. 我们真踩过的坑（都在提交记录里有对应修复）

- 判"文件/进程在不在"要问内核（`test -x`、`pm list packages` 精确等值），
  不要看 `ls` 输出里有没有那个名字——不存在时报错文本也带着那个名字。
- 包名/路径白名单里 `+` 必须在（`libc++_shared.so` 是真实目标），
  校验目的是防注入不是规定命名风格。
- `pm path` 判存在只能认 `package:` 前缀：第三方路径天然带 `==`，系统路径不带 `=`。
- `su -c` 的命令串里不能再出现单引号（`adb::su_wrap` 用单引号包裹，调试构建当场炸）。
- 暂存件要"一次操作一个目录"：备份件与暂存件同级，否则第二次替换会覆盖第一次的备份。
- 两侧对同一个约定（暂存目录形状、字段命名、能力位字符串）必须同源或互相校验，
  只写在文档里的约定会在下一次改动时分叉。
