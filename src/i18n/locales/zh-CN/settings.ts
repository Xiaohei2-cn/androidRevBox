/** 设置页：外观、ADB、工具环境、日志、关于 */
export default {
  "settings.tab.display": "显示",
  "settings.tab.language": "语言",
  "settings.tab.environment": "环境",
  "settings.tab.system": "系统",
  "settings.theme.label": "外观主题",
  "settings.theme.aria": "外观主题",
  "settings.theme.light": "浅色",
  "settings.theme.dark": "深色",
  "settings.theme.system": "跟随系统",
  "settings.theme.hint": "「跟随系统」会实时响应系统深浅色切换",

  "settings.opacity.label": "背景不透明度",
  "settings.opacity.hint":
    "调节窗口背景的不透明度。只是半透明着色，不会模糊背后的内容（WebKit 在透明窗口里模糊不到窗口外的东西，那条实现已删）。最低 20%，保证内容可读。",

  "settings.clickThrough.label": "透明区点击穿透",
  "settings.clickThrough.hint":
    "打开后只有左侧 tab 栏的透明部分（留白、齿块之间的缝隙、未选中齿块那层半透明底）会把点击让给后面的 App；图标有热区、当前 tab 与所有内容区一律照常接住，窗口四边也留着拉伸抓住区，所以界面永远调得动。当前状态见下方实时读数。",

  "settings.clickThrough.status.solid": "当前：光标在实体上，本工具接点击",
  "settings.clickThrough.status.passing": "当前：光标在透明处，点击交给后面的 App",
  "settings.clickThrough.status.outside": "当前：光标不在本窗口内",
  "settings.clickThrough.status.error": "当前：取不到指针位置，已按可点击处理",  "settings.language.label": "界面语言",
  "settings.language.hint": "切换后立即生效并持久化，重启自动恢复。",
  "settings.language.restartHint": "部分系统提示语（如 adb/进程错误）重启后完全生效。",

  "settings.adb.label": "ADB 路径",
  "settings.adb.placeholder": "留空自动探测；或填 adb 可执行文件完整路径",

  "settings.tools.browse": "打开文件选择器",
  "settings.tools.label": "工具环境",
  "settings.tools.hint": "仪表盘环境卡片使用；改动保存后自动重新探测。",
  "settings.tools.pythonPath": "Python 解释器路径",
  "settings.tools.pythonPath.placeholder": "可选 venv/pyenv 解释器；文件选择器会跟随符号链接（选 venv 时自动反查）",
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

  "settings.tab.donate": "捐赠",
  "settings.tab.about": "关于",
  "settings.donate.title": "捐赠",
  "settings.donate.description": "如果这个工具对你有帮助，欢迎支持开发者——捐赠渠道（支付宝/微信/爱发电等）将在此接入。",
} as const;
