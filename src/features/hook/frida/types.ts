import type { FridaEvent } from "@/api/hook";

/** A 区会话设置（启动时快照进会话，§3） */
export interface FridaSettings {
  deviceSerial: string | null;
  connMode: "usb" | "remote";
  /** 远程模式端口（默认 27042） */
  port: string;
  runMode: "attach" | "spawn";
  /** 包名（spawn 必填）或包名/pid（attach） */
  target: string;
}

/** 一个已启动会话（= TaskService 任务，kind="frida"） */
export interface SessionInfo {
  taskId: string;
  name: string;
  startedAt: number;
  script: string;
  settings: FridaSettings;
}

/** 控制台一行：原始行文本（复制用）+ 解析事件 + 时间 */
export interface ConsoleEvent {
  id: number;
  /** Unix 毫秒（实时=到达时间；回放=task_logs.ts 秒 ×1000） */
  ts: number;
  stream: "stdout" | "stderr" | "system";
  line: string;
  evt: FridaEvent;
}

export type TaskState = "pending" | "running" | "success" | "failed" | "cancelled";

/** 过滤条件（持久化 localStorage，按设备+包记忆，§5.3-M1） */
export interface FridaFilter {
  send: boolean;
  log: boolean;
  error: boolean;
  raw: boolean;
  kw: string;
  regex: boolean;
}

export const DEFAULT_FILTER: FridaFilter = {
  send: true,
  log: true,
  error: true,
  raw: true,
  kw: "",
  regex: false,
};

/** 事件是否命中过滤（大小写不敏感；正则坏模式退化为关键词包含） */
export function filterMatch(e: ConsoleEvent, f: FridaFilter): boolean {
  const k = e.evt.kind;
  if (k === "send" && !f.send) return false;
  if (k === "log" && !f.log) return false;
  if (k === "error" && !f.error) return false;
  if ((k === "raw" || k === "exit" || k === "ready") && !f.raw) return false;
  const kw = f.kw.trim();
  if (!kw) return true;
  const text = e.line;
  if (f.regex) {
    try {
      return new RegExp(kw, "i").test(text);
    } catch {
      return text.toLowerCase().includes(kw.toLowerCase());
    }
  }
  return text.toLowerCase().includes(kw.toLowerCase());
}

export function filterStorageKey(settings: Pick<FridaSettings, "deviceSerial" | "target">): string {
  return `hook.frida.filter:${settings.deviceSerial ?? "-"}:${settings.target.trim() || "-"}`;
}

export function loadFilter(key: string): FridaFilter {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return DEFAULT_FILTER;
    const parsed = JSON.parse(raw) as Partial<FridaFilter>;
    return { ...DEFAULT_FILTER, ...parsed };
  } catch {
    return DEFAULT_FILTER;
  }
}

export function saveFilter(key: string, f: FridaFilter): void {
  try {
    localStorage.setItem(key, JSON.stringify(f));
  } catch {
    // 存储不可用静默
  }
}

export const remoteSpec = (s: Pick<FridaSettings, "connMode" | "port">): string | undefined =>
  s.connMode === "remote" ? `127.0.0.1:${s.port.trim() || "27042"}` : undefined;
