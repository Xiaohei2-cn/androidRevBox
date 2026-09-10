/** Настройки: оформление, ADB, инструменты, журнал, о программе */
export default {
  "settings.theme.label": "Оформление",
  "settings.theme.aria": "Тема оформления",
  "settings.theme.light": "Светлая",
  "settings.theme.dark": "Тёмная",
  "settings.theme.system": "Как в системе",
  "settings.theme.hint": "«Как в системе» следует за темой ОС в реальном времени",

  "settings.opacity.label": "Прозрачность фона",
  "settings.opacity.hint":
    "Регулирует прозрачность фона окна с эффектом матового стекла (macOS Vibrancy / Windows Acrylic; в Linux без композитора — просто прозрачность). Минимум 20% сохраняет читаемость.",

  "settings.language.label": "Язык интерфейса",
  "settings.language.hint": "Применяется сразу и сохраняется между запусками.",
  "settings.language.restartHint":
    "Часть системных сообщений (например, ошибки adb/процессов) полностью применится после перезапуска.",

  "settings.adb.label": "Путь к ADB",
  "settings.adb.placeholder": "Пусто = автоопределение; или полный путь к adb",

  "settings.tools.label": "Инструменты",
  "settings.tools.hint": "Используется карточками панели; после сохранения проверка повторится.",
  "settings.tools.pythonPath": "Интерпретатор Python",
  "settings.tools.pythonPath.placeholder":
    "Пусто = не настроено (проверка Frida приостановлена); например /usr/bin/python3",
  "settings.tools.nodePath": "Путь к Node",
  "settings.tools.nodePath.placeholder": "Пусто = поиск node в PATH",
  "settings.tools.idaPort": "Порт IDA MCP",
  "settings.tools.idaPort.placeholder": "по умолчанию 13337",
  "settings.tools.jadxPort": "Порт jadx-gui MCP",
  "settings.tools.jadxPort.placeholder": "по умолчанию 8650",
  "settings.tools.savedReload": "Сохранено; карточки окружения перепроверяются",

  "settings.log.label": "Уровень журнала",
  "settings.log.hint": "Применяется на ходу и сохраняется (SQLite app_settings).",
  "settings.log.syncing": " Синхронизация настроек…",

  "settings.about.label": "О программе",
  "settings.about.appVersion": "Версия приложения",
  "settings.about.tauriVersion": "Версия Tauri",
  "settings.about.platform": "Платформа",
} as const;
