import { stripAnsi } from "@/lib/ansi";
import type { FridaEvent } from "@/api/hook";

/** A 区会话设置（启动时快照进会话，§3） */
export interface FridaSettings {
  deviceSerial: string | null;
  connMode: "usb" | "remote";
  /** 远程模式端口（默认 27042） */
  port: string;
  runMode: "attach" | "spawn";
  /**
   * Spawn：必填，只能是包名。
   * Attach：**留空 = 附加设备当前前台应用**（`frida -UF` 的 -F），填了才按包名/pid 指定。
   */
  target: string;
}

/**
 * 会话的可读名字（任务中心与 C 区标题共用）。
 *
 * attach + 空目标是**合法输入**（= 附加当前前台），但直接拼出来会变成
 * `attach  · hook.js` 这种看着像出错的空位——所以这里补上"自动前台"。
 * 纯函数抽出来是为了能单独钉住：这条规则以前只存在于模板字符串里，没人测过。
 */
export function fridaSessionLabel(
  mode: FridaSettings["runMode"],
  target: string,
  script: string,
): string {
  const who = target.trim() || "自动前台";
  return `${mode} ${who} · ${script}`;
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
  // 过滤与高亮都按"看得见的文字"来：转义序列吃在中间，搜 00000000 就搜不到了
  const text = stripAnsi(e.line);
  if (f.regex) {
    try {
      return new RegExp(kw, "i").test(text);
    } catch {
      return text.toLowerCase().includes(kw.toLowerCase());
    }
  }
  return text.toLowerCase().includes(kw.toLowerCase());
}

/**
 * 「自动换行」开关的持久化键（全局偏好，不按设备分：这是读法偏好，不是上下文）。
 * 关掉换行是为了看宽 hexdump 的列对齐 —— 折行会把一张表切成上下两截。
 */
export const WRAP_STORAGE_KEY = "app.frida.wrap";

export function loadWrap(defaultValue = true): boolean {
  try {
    const raw = localStorage.getItem(WRAP_STORAGE_KEY);
    return raw === null ? defaultValue : raw === "1";
  } catch {
    // localStorage 不可用就不换行偏好，按默认走
    return defaultValue;
  }
}

export function saveWrap(wrap: boolean): void {
  try {
    localStorage.setItem(WRAP_STORAGE_KEY, wrap ? "1" : "0");
  } catch {
    // 写不进去只是下次回到默认，不影响本次阅读
  }
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
