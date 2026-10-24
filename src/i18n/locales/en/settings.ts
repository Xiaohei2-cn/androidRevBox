/** Settings: appearance, ADB, toolchain, logging, about */
export default {
  "settings.tab.display": "Display",
  "settings.tab.language": "Language",
  "settings.tab.environment": "Environment",
  "settings.tab.system": "System",
  "settings.theme.label": "Appearance",
  "settings.theme.aria": "Appearance theme",
  "settings.theme.light": "Light",
  "settings.theme.dark": "Dark",
  "settings.theme.system": "System",
  "settings.theme.hint": "“System” follows the OS theme in real time",

  "settings.agentAuto.label": "Connect the Agent automatically when a device appears",
  "settings.agentAuto.hint":
    "The read-only probe runs on every plug-in. Writing the binary and keeping a process running is confirmed once on the devices page; after that the same device is fully automatic. Turn this off and you are back to connecting by hand.",

  "settings.language.label": "Language",
  "settings.language.hint": "Applies immediately and persists across restarts.",
  "settings.language.restartHint":
    "Some system messages (e.g. adb/process errors) fully apply after a restart.",

  "settings.adb.label": "ADB path",
  "settings.adb.placeholder": "Empty = auto-detect; or full path to the adb executable",

  "settings.tools.browse": "Open file picker",
  "settings.tools.label": "Toolchain",
  "settings.tools.hint": "Used by the dashboard cards; cards re-check after saving.",
  "settings.tools.pythonPath": "Python interpreter",
  "settings.tools.pythonPath.placeholder": "Optional: venv/pyenv interpreter path",
  "settings.tools.nodePath": "Node path",
  "settings.tools.nodePath.placeholder": "Empty = auto-detect node on PATH",
  "settings.tools.fridaTypesDir": "Frida type-library directory",
  "settings.tools.fridaTypesDir.placeholder": "Leave empty to match by frida version",
  "settings.tools.fridaTypesHint": "Where the editor's IntelliSense types come from",
  "settings.tools.idaPort": "IDA MCP port",
  "settings.tools.idaPort.placeholder": "default 13337",
  "settings.tools.jadxPort": "jadx-gui MCP port",
  "settings.tools.jadxPort.placeholder": "default 8650",
  "settings.tools.savedReload": "Saved; re-checking environment cards",

  "settings.log.label": "Log level",
  "settings.log.hint": "Persists across restarts",
  "settings.log.syncing": " Syncing configuration…",

  "settings.about.label": "About",
  "settings.about.appVersion": "App version",
  "settings.about.tauriVersion": "Tauri version",
  "settings.about.platform": "Platform",

  "settings.tab.donate": "Donate",
  "settings.tab.about": "About",
  "settings.donate.title": "Donate",
  "settings.donate.description": "If this tool helps you, consider supporting the developer — donation channels (Alipay / WeChat / Afadian etc.) will be added here.",
  "settings.ai.title": "Translation endpoint (for help probing)",
  "settings.ai.hint": "Optional: translate help text into your UI language",
  "settings.ai.baseUrl": "Endpoint URL (https; local may use http://127.0.0.1)",
  "settings.ai.model": "Model name",
  "settings.ai.key": "API key (stored locally, never shown back)",
  "settings.ai.kept": "Configured (ends {tail}) — blank keeps it",
  "settings.ai.enable": "Enable translation (otherwise the panel shows only the original)",
  "settings.ai.privacy": "Requests go only to the address you set; the API key stays local",
} as const;
