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
  "settings.tools.pythonPath.placeholder": "Pick a venv/pyenv interpreter; the file picker follows symlinks (venv is auto-detected)",
  "settings.tools.nodePath": "Node path",
  "settings.tools.nodePath.placeholder": "Empty = auto-detect node on PATH",
  "settings.tools.idaPort": "IDA MCP port",
  "settings.tools.idaPort.placeholder": "default 13337",
  "settings.tools.jadxPort": "jadx-gui MCP port",
  "settings.tools.jadxPort.placeholder": "default 8650",
  "settings.tools.savedReload": "Saved; re-checking environment cards",

  "settings.log.label": "Log level",
  "settings.log.hint": "Applied at runtime and persisted (SQLite app_settings).",
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
  "settings.ai.hint": "Help text can be translated into the interface language. Optional: the original is always kept.",
  "settings.ai.baseUrl": "Endpoint URL (https; local may use http://127.0.0.1)",
  "settings.ai.model": "Model name",
  "settings.ai.key": "API key (stored locally, never shown back)",
  "settings.ai.kept": "Configured (ends {tail}) — blank keeps it",
  "settings.ai.enable": "Enable translation (otherwise the panel shows only the original)",
  "settings.ai.privacy": "Translated text is sent to the URL you set — never to the device, never logged. The key stays on this machine; the UI only shows its last 4 characters.",
} as const;
