## AppReverseTools（macOS）

Android 逆向工作台。安装包为 ad-hoc 签名、未做 Apple 公证，首次启动需要手动放行一次。

### 安装

1. Apple 芯片下载 `..._aarch64.dmg`，Intel 下载 `..._x64.dmg`
2. 打开 dmg，把 AppReverseTools 拖进「应用程序」
3. 首次启动若提示「无法验证开发者」：右键 App → 打开；
   或执行 `xattr -dr com.apple.quarantine /Applications/AppReverseTools.app`

### 运行依赖

- **adb**（Android platform-tools）：在 PATH 上，或在「设置 → 环境」里指定绝对路径
- **Frida 相关功能**：本机 Python + `pip install frida-tools`
- 设备端 Agent 已随包内置（arm64-v8a），不需要单独下载

### 配置与数据

首次启动使用全新配置，不携带任何开发机设置。配置、日志、插件和工具缓存都在
`~/Library/Application Support/com.appreversetools.app/`，卸载 App 后可自行删除。

### 系统要求

macOS 10.15+，Apple 芯片或 Intel。
