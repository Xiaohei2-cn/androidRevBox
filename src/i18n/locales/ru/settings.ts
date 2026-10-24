/** Настройки: оформление, ADB, инструменты, журнал, о программе */
export default {
  "settings.tab.display": "Оформление",
  "settings.tab.language": "Язык",
  "settings.tab.environment": "Окружение",
  "settings.tab.system": "Система",
  "settings.theme.label": "Оформление",
  "settings.theme.aria": "Тема оформления",
  "settings.theme.light": "Светлая",
  "settings.theme.dark": "Тёмная",
  "settings.theme.system": "Как в системе",
  "settings.theme.hint": "«Как в системе» следует за темой ОС в реальном времени",

  "settings.agentAuto.label": "Автоподключение Agent при появлении устройства",
  "settings.agentAuto.hint":
    "Чтение выполняется при каждом подключении кабеля; запись бинарника и запускresident-процесса подтверждаются один раз на странице устройств, дальше это устройство полностью автоматическое. С выключенным режимом остаётся подключать вручную.",

  "settings.language.label": "Язык интерфейса",
  "settings.language.hint": "Применяется сразу и сохраняется между запусками.",
  "settings.language.restartHint":
    "Часть системных сообщений (например, ошибки adb/процессов) полностью применится после перезапуска.",

  "settings.adb.label": "Путь к ADB",
  "settings.adb.placeholder": "Пусто = автоопределение; или полный путь к adb",

  "settings.tools.browse": "Открыть выбор файла",
  "settings.tools.label": "Инструменты",
  "settings.tools.hint": "Используется карточками панели; после сохранения проверка повторится.",
  "settings.tools.pythonPath": "Интерпретатор Python",
  "settings.tools.pythonPath.placeholder": "Необязательно: путь к интерпретатору venv/pyenv",
  "settings.tools.nodePath": "Путь к Node",
  "settings.tools.nodePath.placeholder": "Пусто = поиск node в PATH",
  "settings.tools.fridaTypesDir": "Каталог типов frida",
  "settings.tools.fridaTypesDir.placeholder": "Оставьте пустым для автоопределения по версии frida",
  "settings.tools.fridaTypesHint": "Откуда редактор берёт типы для подсказок",
  "settings.tools.idaPort": "Порт IDA MCP",
  "settings.tools.idaPort.placeholder": "по умолчанию 13337",
  "settings.tools.jadxPort": "Порт jadx-gui MCP",
  "settings.tools.jadxPort.placeholder": "по умолчанию 8650",
  "settings.tools.savedReload": "Сохранено; карточки окружения перепроверяются",

  "settings.log.label": "Уровень журнала",
  "settings.log.hint": "Сохраняется между запусками",
  "settings.log.syncing": " Синхронизация настроек…",

  "settings.about.label": "О программе",
  "settings.about.appVersion": "Версия приложения",
  "settings.about.tauriVersion": "Версия Tauri",
  "settings.about.platform": "Платформа",

  "settings.tab.donate": "Поддержать",
  "settings.tab.about": "О программе",
  "settings.donate.title": "Поддержать разработчика",
  "settings.donate.description": "Если инструмент вам полезен, поддержите разработчика — каналы донатов (Alipay / WeChat / Boosty и др.) появятся здесь.",
  "settings.ai.title": "Сервис перевода (для проверки справки)",
  "settings.ai.hint": "Необязательно: переводить справку на язык интерфейса",
  "settings.ai.baseUrl": "URL сервиса (https; локально http://127.0.0.1)",
  "settings.ai.model": "Имя модели",
  "settings.ai.key": "API key (хранится локально, обратно не показывается)",
  "settings.ai.kept": "Настроено (оканчивается {tail}) — пустое поле не меняет",
  "settings.ai.enable": "Включить перевод (иначе в панели только оригинал)",
  "settings.ai.privacy": "Запросы идут только на указанный вами адрес; ключ хранится локально",
} as const;
