import { invokeCommand } from "./client";
import { listenEvent } from "./events";

export interface TaskDto {
  id: string;
  taskType: string;
  name: string;
  status: "pending" | "running" | "success" | "failed" | "cancelled";
  exitCode: number | null;
  createdAt: number;
  finishedAt: number | null;
}

export interface TaskLogDto {
  stream: "stdout" | "stderr" | "system";
  chunk: string;
  ts: number;
}

export interface TaskRunArgs {
  executable: string;
  args: string[];
  cwd?: string;
  timeoutMs?: number;
  env?: Record<string, string>;
}

export interface TaskOutputPayload {
  taskId: string;
  stream: TaskLogDto["stream"];
  chunk: string;
}

export interface TaskStatusPayload {
  taskId: string;
  status: TaskDto["status"];
  exitCode: number | null;
}

export const taskApi = {
  /** 起任务：立即返回 task_id，输出走事件流 */
  run(args: TaskRunArgs): Promise<string> {
    return invokeCommand<string>("task_run", { args });
  },
  cancel(id: string): Promise<null> {
    return invokeCommand<null>("task_cancel", { id });
  },
  list(limit = 100): Promise<TaskDto[]> {
    return invokeCommand<TaskDto[]>("task_list", { limit });
  },
  logs(id: string, limit = 1000): Promise<TaskLogDto[]> {
    return invokeCommand<TaskLogDto[]>("task_logs", { id, limit });
  },
  /** 订阅实时输出事件，返回退订函数 */
  onOutput(handler: (p: TaskOutputPayload) => void) {
    return listenEvent<TaskOutputPayload>("task://output", handler);
  },
  /** 订阅任务状态变更，返回退订函数 */
  onStatus(handler: (p: TaskStatusPayload) => void) {
    return listenEvent<TaskStatusPayload>("task://status", handler);
  },
};

/**
 * 把一行 shell 输入拆成 executable + args（不执行任何 shell 展开，
 * 引号支持包裹含空格的参数）。前端只负责解析，执行永远在 Rust CommandSpec。
 */
export function tokenizeCommand(line: string): { executable: string; args: string[] } | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  const tokens: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let has = false;
  for (const ch of trimmed) {
    if (quote) {
      if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      has = true;
    } else if (/\s/.test(ch)) {
      if (has || current) {
        tokens.push(current);
        current = "";
        has = false;
      }
    } else {
      current += ch;
    }
  }
  if (has || current) tokens.push(current);
  if (tokens.length === 0) return null;
  const [executable, ...args] = tokens;
  return { executable, args };
}
