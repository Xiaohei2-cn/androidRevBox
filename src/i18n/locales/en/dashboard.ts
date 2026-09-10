/** Dashboard: environment/tool cards, foreground app card */
export default {
  "dashboard.adb.title": "ADB environment",
  "dashboard.adb.connected": "{count} device(s) connected",
  "dashboard.adb.paused": "Device watching paused",
  "dashboard.adb.others": "({count} more unauthorized/offline)",
  "dashboard.adb.listFailed": "Failed to fetch device list",

  "dashboard.python.title": "Python environment",
  "dashboard.python.detecting": "Checking…",
  "dashboard.python.version": "Python {version}",
  "dashboard.python.notConfiguredHint":
    "No Python interpreter configured: set one in Settings → Toolchain.",
  "dashboard.python.execFailed": "Interpreter failed to run: {error}",
  "dashboard.python.unparsable": "--version ran but output could not be parsed: {output}",

  "dashboard.node.title": "Node environment",
  "dashboard.node.version": "Node {version}",
  "dashboard.node.globalRoot": "global modules",
  "dashboard.node.nodeLabel": "node",
  "dashboard.node.missing":
    "Node not found: install Node.js and add it to PATH, or set the path in Settings → Toolchain.",
  "dashboard.node.execFailed": "Configured node is not executable: {path}",
  "dashboard.node.unparsable": "node --version output could not be parsed: {output}",

  "dashboard.frida.title": "Frida",
  "dashboard.frida.waitPython": "Waiting for Python",
  "dashboard.frida.hintConfig":
    "Configure a working Python interpreter in Settings → Toolchain first",
  "dashboard.frida.notInstalledHint":
    "frida / frida-tools are not installed in this Python environment (pip install frida-tools).",

  "dashboard.ida.title": "IDA MCP",
  "dashboard.jadx.title": "jadx-gui MCP",
  "dashboard.mcp.online": "Online · {port}",
  "dashboard.mcp.unreachable":
    "{name} not detected (connect to {addr} failed: {error}). Start the tool, then refresh.",
  "dashboard.mcp.timeout": "{name} not detected (connect to {addr} timed out).",
  "dashboard.mcp.onlineHint": "{name} service is online ({addr})",

  "dashboard.foreground.title": "Android foreground app",
  "dashboard.foreground.adbUnavailable": "adb unavailable",
  "dashboard.foreground.adbHint": "adb unavailable; foreground detection is paused.",
  "dashboard.foreground.live": "Live",
  "dashboard.foreground.noDevice": "No online device",
  "dashboard.foreground.noDeviceHint":
    "No online device: connect a device/emulator to start detecting.",
  "dashboard.foreground.noForeground": "No foreground app",
  "dashboard.foreground.noForegroundHint":
    "No foreground window parsed (screen may be locked, a dialog may be open, or the OS output differs).",
  "dashboard.foreground.paused": "Paused",
  "dashboard.foreground.package": "Package",
  "dashboard.foreground.activity": "Activity",
  "dashboard.foreground.pid": "PID",
  "dashboard.foreground.nativeLib": "native lib",
  "dashboard.foreground.procPaths": "/proc key paths",
  "dashboard.foreground.noProc": "Process not running (pid unavailable)",
  "dashboard.foreground.unreadable": "unreadable (root may be required)",
  "dashboard.foreground.devicesFailed": "adb devices failed: {error}",
  "dashboard.foreground.windowFailed": "dumpsys window failed: {error}",
} as const;
