import { useEffect, useRef, useState } from "react";
import { taskApi, type TaskLogDto } from "@/api/task";
import { cn } from "@/lib/utils";

export interface LiveLine {
  stream: TaskLogDto["stream"];
  chunk: string;
}

/**
 * 共享任务输出流：实时模式传入 liveLines（事件缓冲），历史模式自动从 DB 拉日志。
 * TasksPage 与设备页的 shell/logcat 等长操作共用。
 */
export function TaskOutputStream({
  taskId,
  running,
  liveLines,
}: {
  taskId: string;
  running: boolean;
  /** 事件缓冲；null = 无缓冲走历史 */
  liveLines: LiveLine[] | null;
}) {
  const [history, setHistory] = useState<LiveLine[] | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (running || liveLines !== null) return;
    let cancelled = false;
    void taskApi
      .logs(taskId)
      .then((rows) =>
        !cancelled && setHistory(rows.map((r) => ({ stream: r.stream, chunk: r.chunk }))),
      )
      .catch(() => !cancelled && setHistory([]));
    return () => {
      cancelled = true;
    };
  }, [taskId, running, liveLines]);

  const lines = liveLines ?? history ?? [];

  useEffect(() => {
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines.length]);

  return (
    <div
      ref={boxRef}
      data-testid="task-output"
      className="h-full min-h-[120px] overflow-auto rounded-lg border bg-black/80 p-3 font-mono text-[11px] leading-relaxed dark:bg-black/40"
    >
      {lines.length === 0 && (
        <span className="text-muted-foreground">
          {running ? "等待输出…" : "（无输出）"}
        </span>
      )}
      {lines.map((l, i) => (
        <div
          key={i}
          className={cn(
            "break-all whitespace-pre-wrap",
            l.stream === "stderr" && "text-red-400",
            l.stream === "system" && "text-amber-400",
          )}
        >
          {l.chunk}
        </div>
      ))}
    </div>
  );
}
