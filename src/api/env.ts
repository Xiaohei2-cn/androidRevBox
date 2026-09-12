import { invokeCommand } from "./client";

/** 与 Rust env_service DTO 对齐（camelCase） */

export interface PythonEnv {
  configured: boolean;
  ready: boolean;
  path?: string | null;
  version?: string | null;
  hint?: string | null;
}

export interface NodeEnv {
  ready: boolean;
  path?: string | null;
  version?: string | null;
  npmGlobalRoot?: string | null;
  hint?: string | null;
}

export interface FridaEnv {
  pythonReady: boolean;
  installed: boolean;
  fridaVersion?: string | null;
  fridaToolsVersion?: string | null;
  hint?: string | null;
}

export interface McpEnv {
  reachable: boolean;
  port: number;
  hint?: string | null;
  /** 宿主应用是否存在（mac 检测；其他平台暂为 null） */
  appInstalled?: boolean | null;
  /** 检测到的应用路径（mac） */
  appPath?: string | null;
}

export interface ProcPath {
  name: string;
  path: string;
  summary?: string | null;
  readable: boolean;
}

export interface ForegroundApp {
  /** ready | adb_unavailable | no_device | no_foreground | error */
  state: string;
  serial?: string | null;
  package?: string | null;
  activity?: string | null;
  pid?: string | null;
  nativeLibDir?: string | null;
  procPaths: ProcPath[];
  hint?: string | null;
  error?: string | null;
}

/** env_resolve_interpreter 返回：picked→resolved 解析结果 */
export interface ResolvedInterpreter {
  pickedPath: string;
  resolvedPath: string;
  version?: string | null;
  /** as-is | pyenv-shim | app-bundle | symlink */
  how: string;
}

export interface EnvOverview {
  python: PythonEnv;
  node: NodeEnv;
  frida: FridaEnv;
  idaMcp: McpEnv;
  jadxMcp: McpEnv;
}

export const envApi = {
  python(): Promise<PythonEnv> {
    return invokeCommand<PythonEnv>("env_python");
  },
  node(): Promise<NodeEnv> {
    return invokeCommand<NodeEnv>("env_node");
  },
  /** Frida 检测依赖 Python 配置，前端在 Python 就绪后才启用该查询（剪枝） */
  frida(): Promise<FridaEnv> {
    return invokeCommand<FridaEnv>("env_frida");
  },
  idaMcp(): Promise<McpEnv> {
    return invokeCommand<McpEnv>("env_ida_mcp");
  },
  jadxMcp(): Promise<McpEnv> {
    return invokeCommand<McpEnv>("env_jadx_mcp");
  },
  /** 安卓前台应用；serial 缺省自动选第一台在线设备；adb 不可用返回 adb_unavailable */
  foreground(serial?: string): Promise<ForegroundApp> {
    return invokeCommand<ForegroundApp>("env_foreground", {
      args: { serial: serial ?? null },
    });
  },
  overview(): Promise<EnvOverview> {
    return invokeCommand<EnvOverview>("env_overview");
  },
  /** 文件选择器选中项 → 真解释器路径（pyenv shim/.app/symlink 解析） */
  resolveInterpreter(picked: string): Promise<ResolvedInterpreter> {
    return invokeCommand<ResolvedInterpreter>("env_resolve_interpreter", { picked });
  },
  /** python 选择器建议起始目录 */
  pythonStartDir(): Promise<string> {
    return invokeCommand<string>("env_python_start_dir");
  },
};
