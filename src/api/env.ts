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
  /** 安卓前台应用；后端 adb 不可用时返回 adb_unavailable（0 次 shell 调用） */
  foreground(): Promise<ForegroundApp> {
    return invokeCommand<ForegroundApp>("env_foreground");
  },
  overview(): Promise<EnvOverview> {
    return invokeCommand<EnvOverview>("env_overview");
  },
};
