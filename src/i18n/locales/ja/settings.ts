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

  "settings.opacity.label": "背景の不透明度",
  "settings.opacity.hint":
    "ウィンドウ背景の不透明度を設定します。半透明の塗りだけなので、背後の内容はぼけません（WebKit は透明ウィンドウの外側をぼかせず、その実装は削除済み）。内容の可読性のため最低 20%。",

  "settings.clickThrough.label": "透明部分をクリック透過",
  "settings.clickThrough.hint":
    "オンにしても後ろのアプリにクリックを渡すのは1箇所だけ：各タブの**左側**の透明な余白です。タブ本体、アイコン、タブ間の隙間、選択中のタブ、コンテンツ領域はすべてクリックを受け取ります。タブをクリックは基本操作なので、後ろに透過させてはいけません。ウィンドウ四辺の外側12pxは常にリサイズ用のつかみ領域です。現在の状態は下のライブ表示に出ます。",

  "settings.clickThrough.status.solid": "現在：ポインターは実体上、本ツールがクリックを受け取ります",
  "settings.clickThrough.status.passing": "現在：ポインターは透明部分上、クリックは後ろのアプリへ渡ります",
  "settings.clickThrough.status.outside": "現在：ポインターはこのウィンドウ外です",
  "settings.clickThrough.status.error": "現在：ポインター位置を取得できないため、クリック可能のままにしています",  "settings.language.label": "表示言語",
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
} as const;
