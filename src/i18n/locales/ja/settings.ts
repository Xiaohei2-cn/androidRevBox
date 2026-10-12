/** 設定：外観、ADB、ツール環境、ログ、このアプリについて */
export default {
  "settings.tab.display": "表示",
  "settings.tab.language": "言語",
  "settings.tab.environment": "環境",
  "settings.tab.system": "システム",
  "settings.theme.label": "外観テーマ",
  "settings.theme.aria": "外観テーマ",
  "settings.theme.light": "ライト",
  "settings.theme.dark": "ダーク",
  "settings.theme.system": "システムに従う",
  "settings.theme.hint": "「システムに従う」は OS のテーマ切替にリアルタイムで追従します",

  "settings.agentAuto.label": "デバイス接続時に Agent を自動接続する",
  "settings.agentAuto.hint":
    "読み取り専用の探测はケーブルを接続する毎回行われます。デバイスへの書き込みと常駐プロセスの起動は、设备画面で一度だけ確認していただき、以降はそのデバイスは完全自動です。オフにすると手動接続のみになります。",

  "settings.language.label": "表示言語",
  "settings.language.hint": "即時反映され、再起動後も保持されます。",
  "settings.language.restartHint":
    "一部のシステムメッセージ（adb／プロセスのエラーなど）は再起動後に完全に反映されます。",

  "settings.adb.label": "ADB のパス",
  "settings.adb.placeholder": "空欄＝自動検出、または adb 実行ファイルのフルパス",

  "settings.tools.browse": "ファイル選択を開く",
  "settings.tools.label": "ツール環境",
  "settings.tools.hint": "ダッシュボードの環境カードが使用します。保存後に再検出します。",
  "settings.tools.pythonPath": "Python インタプリタ",
  "settings.tools.pythonPath.placeholder": "venv／pyenv のインタプリタを選択。ファイル選択はシンボリックリンクを追います（venv は自動判別）",
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

  "settings.tab.donate": "寄付",
  "settings.tab.about": "について",
  "settings.donate.title": "寄付",
  "settings.donate.description": "このツールが役に立ったら開発者を応援してください — 寄付チャネル（Alipay / WeChat / Afdian など）はここに追加されます。",
  "settings.ai.title": "翻訳エンドポイント（ヘルプ探索用）",
  "settings.ai.hint": "ヘルプ文言をUI言語へ翻訳できます。未設定でも使えます：原文は必ず残ります。",
  "settings.ai.baseUrl": "エンドポイント URL（https。ローカルは http://127.0.0.1）",
  "settings.ai.model": "モデル名",
  "settings.ai.key": "API key（本マシンにのみ保存・再表示なし）",
  "settings.ai.kept": "設定済み（末尾 {tail}）· 空欄なら変更しません",
  "settings.ai.enable": "翻訳を有効化（オンにすると探索パネルに訳文が出る）",
  "settings.ai.privacy": "翻訳対象の文言は設定した URL へ送られます（デバイスには送りません、日志にも残しません）。key は本マシンにのみ保存され、画面には末尾 4 文字だけ出ます。",
} as const;
