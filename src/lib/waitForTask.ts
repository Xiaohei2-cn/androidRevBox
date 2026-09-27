import { taskApi, type TaskDto } from "@/api/task";

/** 未结束的状态：轮询时继续等 */
const RUNNING_STATES = new Set(["pending", "running"]);

export type TaskWait = TaskDto["status"] | "timeout" | "unknown";

/**
 * 等一个任务跑到终态（轮询 task_list，不订阅事件）。
 *
 * 为什么轮询而不是 listen：上传/拉取这类"提交完就想立刻知道结果"的入口，
 * 订阅要处理退订、竞态与页面隐藏；一次有界的轮询更好想，也更难写错。
 * `unknown` 表示到超时都没在列表里见到它——只可能发生在任务被裁历史/换了进程，
 * 绝不能当成成功，也不能当成失败：**照实说不知道**。
 */
export async function waitForTask(
  id: string,
  opts: { pollMs?: number; timeoutMs?: number } = {},
): Promise<TaskWait> {
  const pollMs = opts.pollMs ?? 400;
  const timeoutMs = opts.timeoutMs ?? 120_000;
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    // 兜两层：ipc 失败给空数组；调用方 mock/旧版本返回 undefined 也不炸
    const tasks = (await taskApi.list(100).catch(() => [] as TaskDto[])) ?? [];
    const found = tasks.find((t) => t.id === id);
    if (found && !RUNNING_STATES.has(found.status)) return found.status;
    if (Date.now() + pollMs > deadline) return found ? "timeout" : "unknown";
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }
}
