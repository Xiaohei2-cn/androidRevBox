/** Painel: cartões de ambiente e app em primeiro plano */
export default {
  "dashboard.adb.title": "Ambiente ADB",
  "dashboard.adb.connected": "{count} dispositivo(s) conectado(s)",
  "dashboard.adb.paused": "Monitoramento de dispositivos pausado",
  "dashboard.adb.others": "(mais {count} não autorizado(s)/offline)",
  "dashboard.adb.listFailed": "Falha ao obter a lista de dispositivos",

  "dashboard.python.title": "Ambiente Python",
  "dashboard.python.detecting": "Verificando…",
  "dashboard.python.version": "Python {version}",
  "dashboard.python.notConfiguredHint":
    "Nenhum interpretador Python configurado: defina em Configurações → Ferramentas.",
  "dashboard.python.execFailed": "Falha ao executar o interpretador: {error}",
  "dashboard.python.unparsable": "--version executou, mas a saída não pôde ser interpretada: {output}",

  "dashboard.node.title": "Ambiente Node",
  "dashboard.node.version": "Node {version}",
  "dashboard.node.globalRoot": "módulos globais",
  "dashboard.node.nodeLabel": "node",
  "dashboard.node.missing":
    "Node não encontrado: instale o Node.js e adicione ao PATH, ou informe o caminho em Configurações → Ferramentas.",
  "dashboard.node.execFailed": "O node configurado não é executável: {path}",
  "dashboard.node.unparsable": "Saída de node --version não pôde ser interpretada: {output}",

  "dashboard.frida.title": "Frida",
  "dashboard.frida.waitPython": "Aguardando Python",
  "dashboard.frida.hintConfig":
    "Configure um interpretador Python válido em Configurações → Ferramentas",
  "dashboard.frida.notInstalledHint":
    "frida / frida-tools não estão instalados neste ambiente Python (pip install frida-tools).",

  "dashboard.ida.title": "IDA MCP",
  "dashboard.jadx.title": "jadx-gui MCP",
  "dashboard.mcp.online": "Online · {port}",
  "dashboard.mcp.unreachable":
    "{name} não detectado (falha ao conectar em {addr}: {error}). Inicie a ferramenta e atualize.",
  "dashboard.mcp.timeout": "{name} não detectado (tempo esgotado ao conectar em {addr}).",
  "dashboard.mcp.onlineHint": "Serviço {name} online ({addr})",

  "dashboard.foreground.title": "App em primeiro plano (Android)",
  "dashboard.foreground.adbUnavailable": "adb indisponível",
  "dashboard.foreground.adbHint": "adb indisponível; a detecção do app em primeiro plano está pausada.",
  "dashboard.foreground.live": "Ao vivo",
  "dashboard.foreground.noDevice": "Nenhum dispositivo online",
  "dashboard.foreground.noDeviceHint":
    "Nenhum dispositivo online: conecte um dispositivo/emulador para iniciar a detecção.",
  "dashboard.foreground.noForeground": "Nenhum app em primeiro plano",
  "dashboard.foreground.noForegroundHint":
    "Nenhuma janela em primeiro plano reconhecida (tela bloqueada, diálogo aberto ou saída do sistema diferente).",
  "dashboard.foreground.paused": "Pausado",
  "dashboard.foreground.package": "Pacote",
  "dashboard.foreground.activity": "Activity",
  "dashboard.foreground.pid": "PID",
  "dashboard.foreground.nativeLib": "native lib",
  "dashboard.foreground.procPaths": "Caminhos-chave em /proc",
  "dashboard.foreground.noProc": "Processo não está em execução (PID indisponível)",
  "dashboard.foreground.unreadable": "ilegível (pode exigir root)",
  "dashboard.foreground.devicesFailed": "adb devices falhou: {error}",
  "dashboard.foreground.windowFailed": "dumpsys window falhou: {error}",
} as const;
