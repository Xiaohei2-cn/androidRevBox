/** 設定：外観、ADB、ツール環境、ログ、このアプリについて */
export default {
  "settings.theme.label": "外観テーマ",
  "settings.theme.aria": "外観テーマ",
  "settings.theme.light": "ライト",
  "settings.theme.dark": "ダーク",
  "settings.theme.system": "システムに従う",
  "settings.theme.hint": "「システムに従う」は OS のテーマ切替にリアルタイムで追従します",

  "settings.opacity.label": "背景の不透明度",
  "settings.opacity.hint":
    "すりガラス効果を伴うウィンドウ背景の透明度を調整します（macOS Vibrancy / Windows Acrylic、合成器のない Linux では単なる透明度）。最小 20% で可読性を確保します。",

  "settings.language.label": "表示言語",
  "settings.language.hint": "即時反映され、再起動後も保持されます。",
  "settings.language.restartHint":
    "一部のシステムメッセージ（adb／プロセスのエラーなど）は再起動後に完全に反映されます。",

  "settings.adb.label": "ADB のパス",
  "settings.adb.placeholder": "空欄＝自動検出、または adb 実行ファイルのフルパス",

  "settings.tools.label": "ツール環境",
  "settings.tools.hint": "ダッシュボードの環境カードが使用します。保存後に再検出します。",
  "settings.tools.pythonPath": "Python インタプリタ",
  "settings.tools.pythonPath.placeholder":
    "空欄＝未設定（Frida 検出は停止）。例: /usr/bin/python3",
  "settings.tools.nodePath": "Node のパス",
  "settings.tools.nodePath.placeholder": "空欄＝PATH 上の node を自動検出",
  "settings.tools.idaPort": "IDA MCP ポート",
  "settings.tools.idaPort.placeholder": "既定 13337",
  "settings.tools.jadxPort": "jadx-gui MCP ポート",
  "settings.tools.jadxPort.placeholder": "既定 8650",
  "settings.tools.savedReload": "保存しました。環境カードを再検出しています",

  "settings.log.label": "ログレベル",
  "settings.log.hint": "実行時に反映され、永続化されます（SQLite app_settings）。",
  "settings.log.syncing": " 設定を同期中…",

  "settings.about.label": "このアプリについて",
  "settings.about.appVersion": "アプリのバージョン",
  "settings.about.tauriVersion": "Tauri のバージョン",
  "settings.about.platform": "実行プラットフォーム",
} as const;
