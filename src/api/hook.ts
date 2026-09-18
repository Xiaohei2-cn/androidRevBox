import { invokeCommand } from "./client";

/** 与 Rust hook_service DTO 对齐（camelCase）；协议解析在前端（设计 §5.2/§6.3） */

export interface JsFileDto {
  name: string;
  path: string;
  size: number;
  /** Unix 秒 */
  mtime: number;
}

export interface PreflightDto {
  adbOk: boolean;
  adbHint?: string | null;
  pythonOk: boolean;
  pythonHint?: string | null;
  pythonPath?: string | null;
  fridaOk: boolean;
  fridaVersion?: string | null;
  fridaHint?: string | null;
  /** 远程模式探活结果；非远程模式 null（不探） */
  remoteOk: boolean | null;
  remoteHint?: string | null;
  runnerOk: boolean;
  runnerHint?: string | null;
}

export interface HookSessionStartArgs {
  /** USB 模式：adb serial；远程模式 null */
  serial: string | null;
  /** 远程模式：host:port；USB null */
  remote: string | null;
  spawn: boolean;
  /** 包名（spawn）/ 包名或 pid（attach） */
  target: string;
  /** 工作目录下的 .js 文件名 */
  script: string;
}

export const hookApi = {
  /** 扫工作目录一层 *.js；dir 缺省用配置键 app.hook.workdir */
  jsList(dir?: string): Promise<JsFileDto[]> {
    return invokeCommand<JsFileDto[]>("hook_js_list", { dir: dir ?? null });
  },
  /** 前置检查链聚合（adb/python/frida/runner/远程 TCP） */
  preflight(remote?: string): Promise<PreflightDto> {
    return invokeCommand<PreflightDto>("hook_preflight", { remote: remote ?? null });
  },
  /** 起 frida runner 会话任务，返回 taskId；停止=task_cancel，输出=task://output */
  sessionStart(args: HookSessionStartArgs): Promise<string> {
    return invokeCommand<string>("hook_session_start", { args });
  },
};

// ===== runner NDJSON 行协议（§5.2）：纯解析，vitest 覆盖 =====

export type FridaEvent =
  | { kind: "ready"; pid: number | null; mode: string; pkg: string; frida?: string }
  | { kind: "log"; lvl: string; msg: string }
  | { kind: "send"; tag?: string; seq?: number; data: unknown; dataBin?: string }
  | { kind: "error"; why: string; stack?: string | null }
  | { kind: "exit"; code: number | null; why?: string }
  /** 解析失败的行降级为原文（向前兼容 + 脚本手写非协议输出） */
  | { kind: "raw"; text: string };

/**
 * 解析一行 runner 输出 → 事件对象。
 * 非 JSON / 缺 t 字段 / 未知 t → raw 降级；system 流（pid= 等）由调用方标注。
 */
export function parseFridaLine(line: string): FridaEvent {
  const trimmed = line.trim();
  if (!trimmed.startsWith("{")) return { kind: "raw", text: line };
  let obj: unknown;
  try {
    obj = JSON.parse(trimmed);
  } catch {
    return { kind: "raw", text: line };
  }
  if (typeof obj !== "object" || obj === null) return { kind: "raw", text: line };
  const o = obj as Record<string, unknown>;
  switch (o.t) {
    case "ready":
      return {
        kind: "ready",
        pid: typeof o.pid === "number" ? o.pid : null,
        mode: String(o.mode ?? ""),
        pkg: String(o.pkg ?? ""),
        frida: typeof o.frida === "string" ? o.frida : undefined,
      };
    case "log":
      return { kind: "log", lvl: String(o.lvl ?? "log"), msg: String(o.msg ?? "") };
    case "send": {
      const bin = o.data_bin as { _hex?: unknown } | undefined;
      return {
        kind: "send",
        tag: o.tag != null ? String(o.tag) : undefined,
        seq: typeof o.seq === "number" ? o.seq : undefined,
        data: o.data,
        dataBin: typeof bin?._hex === "string" ? bin._hex : undefined,
      };
    }
    case "error":
      return { kind: "error", why: String(o.why ?? "unknown"), stack: o.stack == null ? null : String(o.stack) };
    case "exit":
      return {
        kind: "exit",
        code: typeof o.code === "number" ? o.code : null,
        why: typeof o.why === "string" ? o.why : undefined,
      };
    default:
      return { kind: "raw", text: line };
  }
}

/** send 行摘要（M1 卡片标题用）：`sign("abc…") → "e10a…"` 式的单行压缩 */
export function summarizeEventData(data: unknown): string {
  if (data == null) return "null";
  if (typeof data !== "object") return String(data);
  try {
    return JSON.stringify(data);
  } catch {
    return "[unserializable]";
  }
}
