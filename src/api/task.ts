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

// 分词本体在 `@/lib/commandTokens`（纯逻辑，别处也要用）；这里转出是给既有调用方留的原路径
export { tokenizeCommand } from "@/lib/commandTokens";
