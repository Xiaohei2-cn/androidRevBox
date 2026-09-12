/** ダッシュボード：環境カード／フォアグラウンドアプリ */
export default {
  "dashboard.adb.title": "ADB 環境",
  "dashboard.adb.connected": "接続デバイス {count} 台",
  "dashboard.adb.paused": "デバイス監視は停止中",
  "dashboard.adb.others": "（他 {count} 台が未許可／オフライン）",
  "dashboard.adb.listFailed": "デバイス一覧の取得に失敗しました",

  "dashboard.python.title": "Python 環境",
  "dashboard.python.detecting": "確認中…",
  "dashboard.python.version": "Python {version}",
  "dashboard.python.notConfiguredHint":
    "Python インタプリタが未設定です：設定 → ツール環境 で指定してください。",
  "dashboard.python.execFailed": "インタプリタの実行に失敗: {error}",
  "dashboard.python.unparsable": "--version は実行できましたが出力を解析できません: {output}",

  "dashboard.node.title": "Node 環境",
  "dashboard.node.version": "Node {version}",
  "dashboard.node.globalRoot": "グローバルモジュール",
  "dashboard.node.nodeLabel": "node",
  "dashboard.node.missing":
    "Node が見つかりません：Node.js をインストールして PATH に追加するか、設定 → ツール環境 でパスを指定してください。",
  "dashboard.node.execFailed": "指定された node を実行できません: {path}",
  "dashboard.node.unparsable": "node --version の出力を解析できません: {output}",

  "dashboard.frida.title": "Frida",
  "dashboard.frida.waitPython": "Python の準備待ち",
  "dashboard.frida.hintConfig":
    "先に 設定 → ツール環境 で有効な Python インタプリタを設定してください",
  "dashboard.frida.notInstalledHint":
    "この Python 環境には frida / frida-tools が未インストールです（pip install frida-tools）。",

  "dashboard.ida.title": "IDA MCP",
  "dashboard.jadx.title": "jadx-gui MCP",
  "dashboard.mcp.online": "オンライン · {port}",
  "dashboard.mcp.unreachable":
    "{name} を検出できません（{addr} への接続に失敗: {error}）。ツールを起動して更新してください。",
  "dashboard.mcp.timeout": "{name} を検出できません（{addr} への接続がタイムアウト）。",
  "dashboard.tool.appMissing": "{name} が未インストール",
  "dashboard.tool.mcpLabel": "MCP",
  "dashboard.tool.mcpOnline": "MCP オンライン",
  "dashboard.tool.mcpOffline": "MCP 未起動",
  "dashboard.mcp.onlineHint": "{name} サービスはオンラインです（{addr}）",

  "dashboard.foreground.title": "Android フォアグラウンドアプリ",
  "dashboard.foreground.adbUnavailable": "adb 利用不可",
  "dashboard.foreground.adbHint": "adb が利用できないため、フォアグラウンド検出は停止中です。",
  "dashboard.foreground.live": "リアルタイム検出中",
  "dashboard.foreground.noDevice": "オンラインデバイスなし",
  "dashboard.foreground.noDeviceHint":
    "オンラインデバイスがありません：デバイス／エミュレータを接続すると自動で検出します。",
  "dashboard.foreground.noForeground": "フォアグラウンドアプリなし",
  "dashboard.foreground.noForegroundHint":
    "フォアグラウンドウィンドウを解析できません（画面ロック、ダイアログ、OS 出力差異の可能性）。",
  "dashboard.foreground.paused": "停止中",
  "dashboard.foreground.package": "パッケージ",
  "dashboard.foreground.activity": "Activity",
  "dashboard.foreground.pid": "PID",
  "dashboard.foreground.nativeLib": "native lib",
  "dashboard.foreground.procPaths": "/proc 主要パス",
  "dashboard.foreground.noProc": "プロセスが実行されていません（PID 不明）",
  "dashboard.foreground.unreadable": "読み取り不可（root が必要な場合があります）",
  "dashboard.foreground.devicesFailed": "adb devices 失敗: {error}",
  "dashboard.foreground.windowFailed": "dumpsys window 失敗: {error}",
} as const;
