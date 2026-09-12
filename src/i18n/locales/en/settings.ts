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

  "settings.opacity.label": "Background opacity",
  "settings.opacity.hint":
    "Adjusts window background transparency with a frosted-glass overlay (macOS Vibrancy / Windows Acrylic; falls back to plain transparency on Linux without a compositor). Minimum 20% keeps content readable.",

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
  "settings.tools.pythonPath.placeholder":
    "Empty = not configured (Frida checks pause); e.g. /usr/bin/python3",
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
} as const;
