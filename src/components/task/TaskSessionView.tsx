import { useEffect, useState } from "react";
import { Ban } from "lucide-react";
import { Button } from "@/components/ui/button";
import { taskApi, type TaskStatusPayload } from "@/api/task";
import { TaskOutputStream, type LiveLine } from "./TaskOutputStream";

const MAX_LINES = 2000;

/**
 * 按 taskId 挂接任务：订阅 task://output 实时缓冲 + task://status 终态，
 * 提供取消。运行结束后回退到 DB 历史日志（TaskOutputStream 内部处理）。
 * 设备页一次性操作（安装/卸载/启动）与 shell/logcat 面板共用。
 */
export function TaskSessionView({
  taskId,
  label,
}: {
  taskId: string;
  label?: string;
}) {
  const [status, setStatus] = useState<TaskStatusPayload["status"] | null>("running");
  const [lines, setLines] = useState<LiveLine[]>([]);

  useEffect(() => {
    let alive = true;
    const unsubs: Array<() => void> = [];
    void taskApi
      .onOutput((p) => {
        if (alive && p.taskId === taskId) {
          setLines((prev) => [...prev, { stream: p.stream, chunk: p.chunk }].slice(-MAX_LINES));
        }
      })
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    void taskApi
      .onStatus((p) => alive && p.taskId === taskId && setStatus(p.status))
      .then((un) => alive && unsubs.push(un))
      .catch(() => undefined);
    return () => {
      alive = false;
      unsubs.forEach((un) => un());
    };
  }, [taskId]);

  const isRunning = status === "running" || status === "pending";

  return (
    <div className="flex h-full min-h-0 flex-col gap-1">
      {(label || isRunning) && (
        <div className="flex shrink-0 items-center gap-2">
          {label && <span className="text-xs font-medium">{label}</span>}
          {isRunning && (
            <Button
              size="sm"
              variant="destructive"
              className="ml-auto"
              onClick={() => void taskApi.cancel(taskId)}
            >
              <Ban className="h-3.5 w-3.5" />
              停止
            </Button>
          )}
        </div>
      )}
      <div className="min-h-0 flex-1">
        <TaskOutputStream taskId={taskId} running={isRunning} liveLines={lines} />
      </div>
    </div>
  );
}
