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

  "settings.agentAuto.label": "Conectar o Agent automaticamente quando um dispositivo aparece",
  "settings.agentAuto.hint":
    "A sondagem somente leitura roda a cada conexão de cabo; gravar o binário e manter um processo residente é confirmado uma vez na página de dispositivos — depois aquele dispositivo fica totalmente automático. Desligado, só resta conectar manualmente.",

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
  "settings.tools.pythonPath.placeholder": "Opcional: caminho do interpretador venv/pyenv",
  "settings.tools.nodePath": "Caminho do Node",
  "settings.tools.nodePath.placeholder": "Vazio = detectar node no PATH",
  "settings.tools.fridaTypesDir": "Diretório de tipos do frida",
  "settings.tools.fridaTypesDir.placeholder": "Deixe vazio para corresponder pela versão do frida",
  "settings.tools.fridaTypesHint": "De onde o editor tira os tipos do IntelliSense",
  "settings.tools.idaPort": "Porta do IDA MCP",
  "settings.tools.idaPort.placeholder": "padrão 13337",
  "settings.tools.jadxPort": "Porta do jadx-gui MCP",
  "settings.tools.jadxPort.placeholder": "padrão 8650",
  "settings.tools.savedReload": "Salvo; verificando novamente os cartões de ambiente",

  "settings.log.label": "Nível de log",
  "settings.log.hint": "Persiste após reiniciar",
  "settings.log.syncing": " Sincronizando configuração…",

  "settings.about.label": "Sobre",
  "settings.about.appVersion": "Versão do app",
  "settings.about.tauriVersion": "Versão do Tauri",
  "settings.about.platform": "Plataforma",

  "settings.tab.donate": "Doar",
  "settings.tab.about": "Sobre",
  "settings.donate.title": "Doar",
  "settings.donate.description": "Se esta ferramenta te ajuda, considere apoiar o desenvolvedor — canais de doação (Alipay / WeChat / Afadian etc.) serão adicionados aqui.",
  "settings.ai.title": "Serviço de tradução (para a sonda de ajuda)",
  "settings.ai.hint": "Opcional: traduzir textos de ajuda para o idioma da interface",
  "settings.ai.baseUrl": "URL do serviço (https; local http://127.0.0.1)",
  "settings.ai.model": "Nome do modelo",
  "settings.ai.key": "Chave de API (guardada localmente, não é reexibida)",
  "settings.ai.kept": "Configurada (termina em {tail}) — em branco mantém",
  "settings.ai.enable": "Ativar tradução (senão o painel mostra só o original)",
  "settings.ai.privacy": "As requisições vão apenas para o endereço informado; a chave fica local",
} as const;
