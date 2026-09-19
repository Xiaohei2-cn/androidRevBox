# applist —— Zygisk 模块学习示例

需求：在电脑上用一条命令，获取手机上所有已安装应用的**包名 + 中文显示名 + 版本号**。

## 为什么 adb / pm 拿不到、Zygisk 能拿到

应用的 `android:label`（显示名）通常是字符串资源引用，存放在每个 APK 的
`resources.arsc` 里。只有在应用进程内部通过
`PackageManager.getApplicationLabel()` 才会去真正解析它。
`adb shell pm list packages` 只输出包名；`dumpsys package` 不会为任意包解析显示名。
而 Zygisk 的 **root companion（以 root 身份运行的独立守护进程）** 可以启动
`app_process` 创建一个系统级 Java 进程，通过
`ActivityThread.systemMain().getSystemContext()` 拿到和桌面 Launcher 同级别的
PackageManager —— 这就是本方案的立足点。

## 架构与数据流

```
电脑                         手机
printf 'Q' ──adb forward──> TCP 127.0.0.1:11500
                              ^
                              | (常驻服务循环)
                        root companion 进程  <- REGISTER_ZYGISK_COMPANION 注册
                              | fork 并启动 app_process (环境复制自 zygote64)
                        demo.applist.Helper (Java)
                              | ActivityThread.systemMain()
                              |   .getSystemContext().getPackageManager()
                              |   .getInstalledApplications + getApplicationLabel
                              v
                        一行 JSON -> TCP -> 电脑
```

触发时机：system_server 启动时，模块在 `preServerSpecialize`
（此刻还保有 zygote 特权，SELinux 允许 connect）里调用 `connectCompanion()`
完成握手，把模块安装目录路径发给 companion，点亮上面的 TCP 服务。

## Zygisk 生命周期（对照内核模块）

| 内核模块概念 | 本项目对应 | 代码位置 |
|---|---|---|
| module_init | `REGISTER_ZYGISK_MODULE(ApplistModule)`，生成导出符号 `zygisk_module_entry` | jni/main.cpp 末尾 |
| 装载回调 | `onLoad()` | ApplistModule 类 |
| kprobe 前置处理器 | `preAppSpecialize` / `preServerSpecialize` | ApplistModule 类 |
| 内核线程 kthread | `REGISTER_ZYGISK_COMPANION(companion_handler)`，得到 root 守护进程 | companion 部分 |
| 系统调用传参 | companion socket 上的 `INIT <目录>` 握手文本 | 进程间通信 |
| module_exit | `api->setOption(DLCLOSE_MODULE_LIBRARY)` 主动卸载自身 | 各 pre 阶段回调 |

## 关键坑位（全部实际踩过）

1. **C++ 标准库**：官方 sample 里的 `libcxx` 子模块与较新的 clang 存在兼容问题；
   本项目用不到 string/vector，直接 `APP_STL=none` 并链接系统 `-lstdc++`
   （只为了满足 new 运算符）。头文件用 `<stdio.h>` 等 C 风格写法。
2. **app_process 静默退出（最大的坑）**：手工拼 `ANDROID_ROOT/ANDROID_DATA`
   起不来。ART 缺少 `BOOTCLASSPATH / DEX2OATBOOTCLASSPATH / ANDROID_ART_ROOT /
   ANDROID_I18N_ROOT / ANDROID_TZDATA_ROOT` 任意一个时，**不打任何日志直接以
   退出码 0 结束**，极难排查。
   解法：从 `zygote64` 进程的 `/proc/<pid>/environ` 整份复制环境
   （见 main.cpp 里的 `build_java_env` 函数）。
3. **隐藏接口反射**：独立 `app_process` 没有 targetSdk，反射 `@hide` 的
   ActivityThread 可行；但反射之前必须先 `Looper.prepareMainLooper()`，
   否则 systemMain 内部创建 Handler 时抛异常。
4. **companion 并发**：companion 处理函数可能被多线程并发调用，
   启动常驻服务必须用标志位去重。
5. **connectCompanion 只能在 pre 阶段调用**（SELinux 限制），
   post 阶段所有 Zygisk 接口均已失效。

## 协议 (TCP 127.0.0.1:11500)

| 命令 | 含义 | 响应 |
|---|---|---|
| `Q` | 应用清单 | 一行 JSON: `[{pkg,label,versionName,versionCode}...]` |
| `E` | apk 文件清单(含 split, 不传文件体) | 一行 JSON: `{pkg:[{name,path,size}...]}` |
| `D` | 流式导出 apk | `D` 后每行一个包名(或 `ALL`), 空行结束; 每文件回 `F <size> <pkg> <name>\n` + size 字节裸流, 结束 `DONE\n` |

分包还原原理: split APK 每个都是带独立资源表的完整 APK, **不存在"合并成一个文件"的正确做法**;
正确姿势是 base+splits 一起走一次 `pm install-create/write/commit` 会话,
系统按清单自动装配 —— `get_applist.py install <pkg>` 即此流程的演示.

## 电脑端用法 (get_applist.py)

```bash
python3 get_applist.py list                      # 应用清单 -> apps.json
python3 get_applist.py manifest                  # 全机分包统计
python3 get_applist.py manifest 某包名            # 单包文件清单
python3 get_applist.py export 某包名 [...]       # 导出 base+split 到 apks/<pkg>/ (md5 已验证与源一致)
python3 get_applist.py export --all              # 全机导出
python3 get_applist.py install 某包名            # 导出并装回手机(还原验证)
```

## 构建与使用

```bash
./build.sh                        # 产出 applist.zip
adb push applist.zip /data/local/tmp
adb shell su -c 'ksud module install /data/local/tmp/applist.zip'
# (Magisk 环境改用: magisk --install-module /data/local/tmp/applist.zip)
adb reboot
# 重启后在电脑上 (两种方式):
python3 get_applist.py              # 客户端脚本: 自动 forward + 查询 + 保存 apps.json
# 或手工:
adb forward tcp:11500 tcp:11500
(printf 'Q'; sleep 3) | nc 127.0.0.1 11500 | python3 -m json.tool
# 注意: macOS 上直接 printf 'Q' | nc 会拿到空结果 —— nc 在 stdin 结束时
# 立即断开, 而模块查询需要约 1 秒. 用 sleep 保住连接即可.
```

## 免重启热更新 helper

`.so` 是开机时注入 system_server 的, 改 C++ 必须重装+重启;
但 `helper.dex` 是**每次查询时才新起进程加载**的, 改 Java 只需替换文件:

```bash
adb shell su -c 'cp /data/local/tmp/classes.dex /data/adb/modules/applist/helper.dex'
python3 get_applist.py   # 立即生效
```

返回的每条记录包含四个字段: pkg / label / versionName / versionCode.

## 目录结构

```
jni/main.cpp        模块主体: 生命周期类 + companion 服务 (TCP + 启动 app_process)
jni/zygisk.hpp      官方 ABI 头文件 (禁止修改)
helper/src/...      Java 助手: 反射获取系统 Context, 解析应用显示名
pkg/                模块压缩包骨架 (module.prop / zygisk/*.so / helper.dex / 安装脚本)
build.sh            ndk-build + javac/d8 + zip 一键打包
```
