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
    "Sets how opaque the window background is. It is a translucent fill only — nothing behind the window gets blurred (WebKit cannot blur outside a transparent window, and that implementation was removed). Minimum 20% so content stays readable.",

  "settings.clickThrough.label": "Click through transparent areas",
  "settings.clickThrough.hint":
    "When on, only the transparent parts of the left tab rail (its gutter, the gaps between teeth and the translucent teeth themselves) hand clicks to the app behind. Tab icons keep a hit area, the active tab and the whole content area always keep the clicks, and the window edges stay grabbable so you can still resize it. The live readout below shows the current state.",

  "settings.clickThrough.status.solid": "Now: pointer is over real content, this tool keeps the clicks",
  "settings.clickThrough.status.passing": "Now: pointer is over a transparent spot, clicks go to the app behind",
  "settings.clickThrough.status.outside": "Now: the pointer is outside this window",
  "settings.clickThrough.status.error": "Now: pointer position unavailable, kept the window clickable",  "settings.language.label": "Language",
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
} as const;
