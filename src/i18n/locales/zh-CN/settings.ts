/** 设置页：外观、ADB、工具环境、日志、关于 */
export default {
  "settings.theme.label": "外观主题",
  "settings.theme.aria": "外观主题",
  "settings.theme.light": "浅色",
  "settings.theme.dark": "深色",
  "settings.theme.system": "跟随系统",
  "settings.theme.hint": "「跟随系统」会实时响应系统深浅色切换",

  "settings.opacity.label": "背景不透明度",
  "settings.opacity.hint":
    "调节窗口背景透明度，并叠加毛玻璃效果（macOS Vibrancy / Windows Acrylic；Linux 无合成器时自动降级为纯透明度）。最低 20%，保证内容可读。",

  "settings.language.label": "界面语言",
  "settings.language.hint": "切换后立即生效并持久化，重启自动恢复。",
  "settings.language.restartHint": "部分系统提示语（如 adb/进程错误）重启后完全生效。",

  "settings.adb.label": "ADB 路径",
  "settings.adb.placeholder": "留空自动探测；或填 adb 可执行文件完整路径",

  "settings.tools.label": "工具环境",
  "settings.tools.hint": "仪表盘环境卡片使用；改动保存后自动重新探测。",
  "settings.tools.pythonPath": "Python 解释器路径",
  "settings.tools.pythonPath.placeholder":
    "留空 = 未配置（Frida 检测将暂停）；如 /usr/bin/python3",
  "settings.tools.nodePath": "Node 路径",
  "settings.tools.nodePath.placeholder": "留空 = 自动探测系统 PATH 上的 node",
  "settings.tools.idaPort": "IDA MCP 端口",
  "settings.tools.idaPort.placeholder": "默认 13337",
  "settings.tools.jadxPort": "jadx-gui MCP 端口",
  "settings.tools.jadxPort.placeholder": "默认 8650",
  "settings.tools.savedReload": "已保存，环境卡重新检测中",

  "settings.log.label": "日志级别",
  "settings.log.hint": "运行时生效并持久化（SQLite app_settings），重启自动恢复。",
  "settings.log.syncing": " 配置同步中…",

  "settings.about.label": "关于",
  "settings.about.appVersion": "应用版本",
  "settings.about.tauriVersion": "Tauri 版本",
  "settings.about.platform": "运行平台",
} as const;
