/** Configurações: aparência, ADB, ferramentas, log, sobre */
export default {
  "settings.theme.label": "Aparência",
  "settings.theme.aria": "Tema da aparência",
  "settings.theme.light": "Claro",
  "settings.theme.dark": "Escuro",
  "settings.theme.system": "Sistema",
  "settings.theme.hint": "“Sistema” segue o tema do SO em tempo real",

  "settings.opacity.label": "Opacidade do fundo",
  "settings.opacity.hint":
    "Ajusta a transparência do fundo da janela com efeito de vidro fosco (macOS Vibrancy / Windows Acrylic; no Linux sem compositor, apenas transparência). O mínimo de 20% mantém o conteúdo legível.",

  "settings.language.label": "Idioma da interface",
  "settings.language.hint": "Aplica imediatamente e persiste entre reinícios.",
  "settings.language.restartHint":
    "Algumas mensagens do sistema (ex.: erros de adb/processo) só se aplicam por completo após reiniciar.",

  "settings.adb.label": "Caminho do ADB",
  "settings.adb.placeholder": "Vazio = detecção automática; ou caminho completo do adb",

  "settings.tools.label": "Ferramentas",
  "settings.tools.hint": "Usado pelos cartões do painel; a verificação é refeita após salvar.",
  "settings.tools.pythonPath": "Interpretador Python",
  "settings.tools.pythonPath.placeholder":
    "Vazio = não configurado (verificação do Frida pausada); ex.: /usr/bin/python3",
  "settings.tools.nodePath": "Caminho do Node",
  "settings.tools.nodePath.placeholder": "Vazio = detectar node no PATH",
  "settings.tools.idaPort": "Porta do IDA MCP",
  "settings.tools.idaPort.placeholder": "padrão 13337",
  "settings.tools.jadxPort": "Porta do jadx-gui MCP",
  "settings.tools.jadxPort.placeholder": "padrão 8650",
  "settings.tools.savedReload": "Salvo; verificando novamente os cartões de ambiente",

  "settings.log.label": "Nível de log",
  "settings.log.hint": "Aplicado em tempo de execução e persistido (SQLite app_settings).",
  "settings.log.syncing": " Sincronizando configuração…",

  "settings.about.label": "Sobre",
  "settings.about.appVersion": "Versão do app",
  "settings.about.tauriVersion": "Versão do Tauri",
  "settings.about.platform": "Plataforma",
} as const;
