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

  "settings.opacity.label": "Прозрачность фона",
  "settings.opacity.hint":
    "Задаёт непрозрачность фона окна. Это лишь полупрозрачная заливка — то, что за окном, не размывается (WebKit не размывает содержимое за пределами прозрачного окна, эта реализация удалена). Минимум 20%, чтобы текст оставался читаемым.",

  "settings.clickThrough.label": "Клики сквозь прозрачные области",
  "settings.clickThrough.hint":
    "Когда включено, клики уходят позади окна только из прозрачных частей панели вкладок слева (поля, промежутки между «зубцами» и сами полупрозрачные «зубцы»). По иконкам остаётся область попадания, активная вкладка и вся область контента по-прежнему принимают клики, а края окна остаются зоной захвата — размер можно менять. Текущее состояние показано ниже.",

  "settings.clickThrough.status.solid": "Сейчас: курсор на содержимом — клики остаются здесь",
  "settings.clickThrough.status.passing": "Сейчас: курсор на прозрачном месте — клики уходят позади окна",
  "settings.clickThrough.status.outside": "Сейчас: курсор вне этого окна",
  "settings.clickThrough.status.error": "Сейчас: позицию курсора не получить, окно осталось кликабельным",  "settings.language.label": "Язык интерфейса",
  "settings.language.hint": "Применяется сразу и сохраняется между запусками.",
  "settings.language.restartHint":
    "Часть системных сообщений (например, ошибки adb/процессов) полностью применится после перезапуска.",

  "settings.adb.label": "Путь к ADB",
  "settings.adb.placeholder": "Пусто = автоопределение; или полный путь к adb",

  "settings.tools.browse": "Открыть выбор файла",
  "settings.tools.label": "Инструменты",
  "settings.tools.hint": "Используется карточками панели; после сохранения проверка повторится.",
  "settings.tools.pythonPath": "Интерпретатор Python",
  "settings.tools.pythonPath.placeholder": "Выберите интерпретатор venv/pyenv; выбор файла следует по симлинкам (venv определяется автоматически)",
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

  "settings.tab.donate": "Поддержать",
  "settings.tab.about": "О программе",
  "settings.donate.title": "Поддержать разработчика",
  "settings.donate.description": "Если инструмент вам полезен, поддержите разработчика — каналы донатов (Alipay / WeChat / Boosty и др.) появятся здесь.",
} as const;
