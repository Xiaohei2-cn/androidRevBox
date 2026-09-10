/** Панель: карточки окружения и активного приложения */
export default {
  "dashboard.adb.title": "Окружение ADB",
  "dashboard.adb.connected": "Подключено устройств: {count}",
  "dashboard.adb.paused": "Отслеживание устройств приостановлено",
  "dashboard.adb.others": "(ещё {count} неавторизованных/офлайн)",
  "dashboard.adb.listFailed": "Не удалось получить список устройств",

  "dashboard.python.title": "Окружение Python",
  "dashboard.python.detecting": "Проверка…",
  "dashboard.python.version": "Python {version}",
  "dashboard.python.notConfiguredHint":
    "Интерпретатор Python не настроен: укажите его в Настройки → Инструменты.",
  "dashboard.python.execFailed": "Не удалось запустить интерпретатор: {error}",
  "dashboard.python.unparsable": "--version выполнен, но вывод не распознан: {output}",

  "dashboard.node.title": "Окружение Node",
  "dashboard.node.version": "Node {version}",
  "dashboard.node.globalRoot": "глобальные модули",
  "dashboard.node.nodeLabel": "node",
  "dashboard.node.missing":
    "Node не найден: установите Node.js и добавьте в PATH либо укажите путь в Настройки → Инструменты.",
  "dashboard.node.execFailed": "Указанный node не запускается: {path}",
  "dashboard.node.unparsable": "Вывод node --version не распознан: {output}",

  "dashboard.frida.title": "Frida",
  "dashboard.frida.waitPython": "Ожидание Python",
  "dashboard.frida.hintConfig":
    "Сначала настройте рабочий интерпретатор Python в Настройки → Инструменты",
  "dashboard.frida.notInstalledHint":
    "В этом окружении Python нет frida / frida-tools (pip install frida-tools).",

  "dashboard.ida.title": "IDA MCP",
  "dashboard.jadx.title": "jadx-gui MCP",
  "dashboard.mcp.online": "В сети · {port}",
  "dashboard.mcp.unreachable":
    "{name} не обнаружен (подключение к {addr} не удалось: {error}). Запустите инструмент и обновите.",
  "dashboard.mcp.timeout": "{name} не обнаружен (тайм-аут подключения к {addr}).",
  "dashboard.mcp.onlineHint": "Служба {name} доступна ({addr})",

  "dashboard.foreground.title": "Активное приложение Android",
  "dashboard.foreground.adbUnavailable": "adb недоступен",
  "dashboard.foreground.adbHint": "adb недоступен, определение активного приложения приостановлено.",
  "dashboard.foreground.live": "В реальном времени",
  "dashboard.foreground.noDevice": "Нет устройств в сети",
  "dashboard.foreground.noDeviceHint":
    "Нет устройств в сети: подключите устройство или эмулятор.",
  "dashboard.foreground.noForeground": "Нет активного приложения",
  "dashboard.foreground.noForegroundHint":
    "Активное окно не распознано (возможно, экран заблокирован, открыт диалог или отличается вывод ОС).",
  "dashboard.foreground.paused": "Пауза",
  "dashboard.foreground.package": "Пакет",
  "dashboard.foreground.activity": "Activity",
  "dashboard.foreground.pid": "PID",
  "dashboard.foreground.nativeLib": "native lib",
  "dashboard.foreground.procPaths": "Ключевые пути /proc",
  "dashboard.foreground.noProc": "Процесс не запущен (PID недоступен)",
  "dashboard.foreground.unreadable": "недоступно для чтения (возможно, нужен root)",
  "dashboard.foreground.devicesFailed": "adb devices: ошибка {error}",
  "dashboard.foreground.windowFailed": "dumpsys window: ошибка {error}",
} as const;
