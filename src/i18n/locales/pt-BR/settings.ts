/** Configurações: aparência, ADB, ferramentas, log, sobre */
export default {
  "settings.tab.display": "Exibição",
  "settings.tab.language": "Idioma",
  "settings.tab.environment": "Ambiente",
  "settings.tab.system": "Sistema",
  "settings.theme.label": "Aparência",
  "settings.theme.aria": "Tema da aparência",
  "settings.theme.light": "Claro",
  "settings.theme.dark": "Escuro",
  "settings.theme.system": "Sistema",
  "settings.theme.hint": "“Sistema” segue o tema do SO em tempo real",

  "settings.opacity.label": "Opacidade do fundo",
  "settings.opacity.hint":
    "Define a opacidade do fundo da janela. É apenas um preenchimento translúcido — nada atrás da janela fica desfocado (o WebKit não desboca fora de uma janela transparente e essa implementação foi removida). Mínimo 20% para manter o conteúdo legível.",

  "settings.clickThrough.label": "Clicar através das áreas transparentes",
  "settings.clickThrough.hint":
    "Quando ativo, cliques em pontos realmente transparentes (vãos entre cartões, margens, fora dos cantos arredondados) vão para o app atrás desta janela. Cartões, botões, campos e texto continuam pertencendo a esta ferramenta. A barra de título e a barra de abas ficam sempre clicáveis, então dá para voltar aqui e desligar.",
  "settings.language.label": "Idioma da interface",
  "settings.language.hint": "Aplica imediatamente e persiste entre reinícios.",
  "settings.language.restartHint":
    "Algumas mensagens do sistema (ex.: erros de adb/processo) só se aplicam por completo após reiniciar.",

  "settings.adb.label": "Caminho do ADB",
  "settings.adb.placeholder": "Vazio = detecção automática; ou caminho completo do adb",

  "settings.tools.browse": "Abrir seletor de arquivos",
  "settings.tools.label": "Ferramentas",
  "settings.tools.hint": "Usado pelos cartões do painel; a verificação é refeita após salvar.",
  "settings.tools.pythonPath": "Interpretador Python",
  "settings.tools.pythonPath.placeholder": "Escolha um interpretador venv/pyenv; o seletor segue symlinks (venv é detectado automaticamente)",
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

  "settings.tab.donate": "Doar",
  "settings.tab.about": "Sobre",
  "settings.donate.title": "Doar",
  "settings.donate.description": "Se esta ferramenta te ajuda, considere apoiar o desenvolvedor — canais de doação (Alipay / WeChat / Afadian etc.) serão adicionados aqui.",
} as const;
