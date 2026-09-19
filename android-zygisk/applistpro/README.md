# ApplistPro —— 自有 Zygisk 模块 v2 基线

与 `android-zygisk/applist`（用户提供的学习 demo）**并存、互不覆盖**：module id 是
`applistpro`、端口是 `11501`。demo 目录与模板接线文件保持零改动，本模块是为了解决
demo 结构性受限而新建的自有实现（阶段文档 AR5.6 / D016~D018）。

## 为什么要另起一个模块

| demo 的限制 | v2 的处理 |
|---|---|
| TCP `127.0.0.1:11500` 无任何鉴权，设备内任意进程都能读全机应用清单和任意 APK | 必须先用一次性令牌握手（`H 2 <token>`），令牌是模块启动时生成的 64 字节随机值，只存在 root-only `0600` 文件里；未鉴权连接最多 3 次尝试即断开 |
| 只能返回「设备当前 locale」的名称，指定语言无从下手 | `L` 命令带 `locale` 参数：按包的 `AssetManager` 构造局部 `Resources`，并回填 `resolvedLocale`；拿不到就返回 `fallbackReason`，不拿包名冒充本地化名称 |
| 一行 JSON 受 4 MiB 响应缓冲限制，包数增长会截断 | 响应改成 NDJSON + 4 字节大端长度前缀分帧，末帧 `{"final":true,...}` 带计数 |
| 只有 `pkg/label/版本` 四个字段 | 增加 `uid / isSystem / enabled / labelSource / requestedLocale / resolvedLocale / fallbackReason`，`scope` 与 `include_disabled` 在设备端过滤 |
| 未知命令一律按单字符处理，无版本协商 | 握手返回 `协议版本 + 模块版本 + versionCode + 设备 locale + 能力列表`，版本不匹配直接 `ERR unsupported_protocol` |

## 架构

```
Android Agent (root 读 token)                    手机
  --TCP 127.0.0.1:11501--> root companion 服务进程（单线程串行，避免 helper 冷启动互踩）
                             | fork+exec app_process（环境整份复制自 zygote64 /proc/<pid>/environ）
                           pro.applist.HelperPro (Java, 系统级进程)
                             | ActivityThread.systemMain().getSystemContext().getPackageManager()
                             | 局部 Resources 按 locale 解析 label
                             v
                    NDJSON 分帧 -> TCP -> Agent -> 公共 DTO -> Desktop
```

Desktop **不直连**本模块端口：Agent 的 `ZygiskProvider` 负责私有协议，公共 typed API 才对外
（阶段文档 §3.4.1 第 4 条）。APK 导出仍由 Agent 写入设备侧 `0700` 暂存目录后走 ADB pull。

## 线协议 v2（子协议版本 `2`）

未鉴权前只接受握手：

```
C: H 2 <128hex>\n
M: OK 2 <version> <versionCode> <deviceLocale> list manifest export\n
   ERR auth_required | bad_handshake | unsupported_protocol | auth_failed\n   (随后断开)
```

握手成功后同一连接可反复发命令（30 s 空闲自动关闭）：

```
C: S\n                                   # 状态/能力再确认
M: STATUS 2 <version> <versionCode> <deviceLocale> list manifest export\n

C: L <locale|-> <all|user|system> <0|1>\n
M: <4B len><item json> ... <4B len>{"final":true,"count":N,"fallback":M,"localeUnproven":K,"deviceLocale":"...","enumeratedFlags":F}
   ERR helper_failed | bad_request | bad_locale | bad_scope | too_many_items\n

   item: {"pkg","label","labelSource":"framework|package_name|manifest",
          "requestedLocale","resolvedLocale"|null,"fallbackReason"|null,
          "versionName","versionCode","uid","isSystem","enabled"}

C: M\n                                   # 每包 base+split 文件清单（不传文件体）
M: <4B len>{"pkg":"...","files":[{"name","path","size"}...]} ... <4B len>{"final":true,"count":N}

C: E <pkg>\n                             # 流式导出；包名拒绝 shell 元字符与路径分隔
M: F <size> <pkg> <name>\n <size 字节裸流> ...
   T <size> <pkg> <name> too_large\n      # 超过单文件 512 MiB 上限时跳过并标注
   DONE\n                                 # 结束（一个文件都没命中时改回 `ERR no_files <pkg>`，不回空 DONE）
   ERR <code>[ <消息>]\n                    # 异常时先 ERR 再断开；无消息时会留一个尾随空格，
                                            # 客户端必须按空白切分而不是整串等值比较

C: X\n                                   # 主动结束会话
```

错误码：`auth_required` `auth_failed` `bad_handshake` `unsupported_protocol` `bad_request`
`bad_locale` `bad_scope` `bad_package` `unknown_command` `helper_failed` `no_files`
`too_many_items` `export_truncated` `too_large`。

限制：单次 helper 输出上限 8 MiB（超过即 `ERR helper_failed`，不返回半截清单）、
条目上限 200000 行、导出总量上限 512 MiB。

## 构建

```bash
./build.sh          # 产出 applistpro.zip（依赖 NDK 28.2 / build-tools 36.1 / android-36.1 platform）
```

产物 `pkg/zygisk/arm64-v8a.so`、`pkg/helper.dex`、`applistpro.zip` 均为可再生缓存，已在
`.gitignore` 里排除，仓库只跟踪源码与接线模板。

## 安装（需要重启，务必先确认可以承担启动风险）

```bash
adb push applistpro.zip /data/local/tmp
adb shell su -c 'ksud module install /data/local/tmp/applistpro.zip'
adb reboot
# 重启后：模块在 <module_dir>/token 生成令牌；禁用用 `ksud module disable applistpro`
```

与 demo 模块可同时存在（`applist` 11500 / `applistpro` 11501），迁移完成后再决定是否停用 demo。

## 尚未验证（必须真机确认，不能凭编译通过当成功）

1. ~~局部 Resources 按指定 locale 解析~~ **已真机验证**：同机 `zh-CN/en-US/fr-FR/ja-JP` 对
   `com.google.android.apps.weather` 分别返回 天气/Weather/Météo/天気情報。同时验证出
   `updateConfiguration` 回填的是**请求值而非实际命中值**（无 fr 资源的包退回中文却自称
   `fr-FR`），因此 `resolvedLocale` 改成只在「与设备默认解析不同」时上报，否则为 `null`
   并标 `locale_not_resolved_fallback_default`。
2. `MATCH_DISABLED_COMPONENTS | MATCH_UNINSTALLED_PACKAGES` 是否真能枚举出停用应用并拿到 label
   （demo 的 `Q` 枚举不到，这是 AR5.4 遗留缺口）。
3. companion 生成/读取 `0600` 令牌文件的 SELinux 上下文，以及 `app_process` 冷启动在
   变体模块下仍然可用（环境整份复制自 zygote64 是 demo 踩出来的坑，这里沿用）。
4. 与 `zygisk-maphide`、`Shamiko` 等隐藏类模块共存时的行为（模块目录 readlink 是否仍可信）。
