import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ban, Play, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { taskApi, tokenizeCommand, type TaskDto, type TaskLogDto } from "@/api/task";
import { cn } from "@/lib/utils";

const MAX_LINES = 2000;

interface LiveLine {
  stream: TaskLogDto["stream"];
  chunk: string;
}

/** 任务中心（P2）：发起命令、实时 stdout/stderr、取消、历史与日志 */
export function TasksPage() {
  const queryClient = useQueryClient();
  const [commandLine, setCommandLine] = useState("");
  const [timeoutSec, setTimeoutSec] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  /** taskId → 实时输出缓冲（事件流追加，选中切换不清空，任务重开时清空） */
  const [live, setLive] = useState<Record<string, LiveLine[]>>({});

  const { data: tasks = [] } = useQuery({
    queryKey: ["tasks", "list"],
    queryFn: () => taskApi.list(100),
    refetchInterval: (q) =>
      q.state.data?.some((t) => t.status === "running" || t.status === "pending")
        ? 1500
        : false,
  });

  const selected = useMemo(
    () => tasks.find((t) => t.id === selectedId) ?? null,
    [tasks, selectedId],
  );

  const refreshList = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ["tasks", "list"] });
  }, [queryClient]);

  // 全局订阅：输出事件进缓冲，状态事件驱动列表刷新
  useEffect(() => {
    const unsubs: Array<() => void> = [];
    let alive = true;
    const subscribe = (p: Promise<() => void>) => {
      p.then((un) => {
        if (alive) unsubs.push(un);
        else un();
      }).catch(() => {
        /* 非 Tauri 环境/订阅失败：忽略，列表轮询仍可展示状态 */
      });
    };
    subscribe(
      taskApi.onOutput((p) => {
        if (!alive) return;
        setLive((prev) => {
          const arr = prev[p.taskId] ?? [];
          const next = [...arr, { stream: p.stream, chunk: p.chunk }];
          return { ...prev, [p.taskId]: next.slice(-MAX_LINES) };
        });
      }),
    );
    subscribe(taskApi.onStatus(() => refreshList()));
    return () => {
      alive = false;
      unsubs.forEach((un) => un());
    };
  }, [refreshList]);

  const runMutation = useMutation({
    mutationFn: async () => {
      const parsed = tokenizeCommand(commandLine);
      if (!parsed) throw new Error("请输入命令");
      const timeoutMs =
        timeoutSec && Number(timeoutSec) > 0
          ? Math.min(3_600_000, Math.round(Number(timeoutSec) * 1000))
          : undefined;
      return taskApi.run({ executable: parsed.executable, args: parsed.args, timeoutMs });
    },
    onSuccess: (id) => {
      setLive((prev) => ({ ...prev, [id]: [] }));
      setSelectedId(id);
      refreshList();
    },
  });

  const cancelMutation = useMutation({
    mutationFn: (id: string) => taskApi.cancel(id),
    onSuccess: refreshList,
  });

  const selectedRunning =
    selected?.status === "running" || selected?.status === "pending";

  return (
    <div className="flex h-full gap-4">
      <section className="flex w-[340px] shrink-0 flex-col gap-2">
        <div className="flex flex-col gap-2 rounded-lg border p-3">
          <Label htmlFor="task-cmd">执行命令</Label>
          <Input
            id="task-cmd"
            data-testid="task-cmd"
            placeholder="如：ping -c 4 127.0.0.1"
            value={commandLine}
            onChange={(e) => setCommandLine(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !runMutation.isPending) runMutation.mutate();
            }}
          />
          <div className="flex items-center gap-2">
            <Input
              data-testid="task-timeout"
              placeholder="超时秒（可选）"
              className="w-32"
              type="number"
              min={0.1}
              value={timeoutSec}
              onChange={(e) => setTimeoutSec(e.target.value)}
            />
            <Button
              data-testid="task-run"
              size="sm"
              className="ml-auto"
              disabled={runMutation.isPending}
              onClick={() => runMutation.mutate()}
            >
              {runMutation.isPending ? (
                <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Play className="h-3.5 w-3.5" />
              )}
              运行
            </Button>
          </div>
          {runMutation.error && (
            <p className="text-xs text-destructive">
              {String((runMutation.error as Error).message)}
            </p>
          )}
        </div>

        <div className="min-h-0 flex-1 overflow-auto rounded-lg border">
          {tasks.length === 0 && (
            <p className="p-4 text-center text-xs text-muted-foreground">
              暂无任务，输入命令开始第一个
            </p>
          )}
          <ul className="divide-y">
            {tasks.map((t) => (
              <TaskListItem
                key={t.id}
                task={t}
                selected={t.id === selectedId}
                onClick={() => setSelectedId(t.id)}
              />
            ))}
          </ul>
        </div>
      </section>

      <section className="flex min-w-0 flex-1 flex-col gap-2">
        {!selected ? (
          <div className="flex h-full items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground">
            选择左侧任务查看输出
          </div>
        ) : (
          <>
            <div className="flex items-center gap-3">
              <span className="truncate text-sm font-medium">{selected.name}</span>
              <StatusBadge status={selected.status} />
              {selected.exitCode !== null && (
                <span className="text-xs text-muted-foreground">
                  退出码 {selected.exitCode}
                </span>
              )}
              <span className="text-xs tabular-nums text-muted-foreground">
                耗时 {formatDuration(selected.createdAt, selected.finishedAt)}
              </span>
              {selectedRunning && (
                <Button
                  variant="destructive"
                  size="sm"
                  className="ml-auto"
                  disabled={cancelMutation.isPending}
                  onClick={() => cancelMutation.mutate(selected.id)}
                >
                  <Ban className="h-3.5 w-3.5" />
                  取消
                </Button>
              )}
            </div>
            <Tabs defaultValue="output" className="flex min-h-0 flex-1 flex-col">
              <TabsList className="w-fit shrink-0">
                <TabsTrigger value="output">输出</TabsTrigger>
                <TabsTrigger value="info">信息</TabsTrigger>
              </TabsList>
              <TabsContent value="output" className="min-h-0 flex-1">
                <OutputView
                  key={selected.id}
                  task={selected}
                  running={selectedRunning}
                  liveLines={live[selected.id] ?? null}
                  onError={(msg) =>
                    setLive((prev) => ({
                      ...prev,
                      [selected.id]: [...(prev[selected.id] ?? []), { stream: "system", chunk: msg }],
                    }))
                  }
                />
              </TabsContent>
              <TabsContent value="info" className="min-h-0 flex-1 overflow-auto">
                <InfoView task={selected} />
              </TabsContent>
            </Tabs>
          </>
        )}
      </section>
    </div>
  );
}

function TaskListItem({
  task,
  selected,
  onClick,
}: {
  task: TaskDto;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          "flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition-colors hover:bg-accent",
          selected && "bg-accent",
        )}
      >
        <StatusDot status={task.status} />
        <span className="min-w-0 flex-1 truncate">{task.name}</span>
        <span className="shrink-0 text-muted-foreground">
          {formatTime(task.createdAt)}
        </span>
      </button>
    </li>
  );
}

function StatusDot({ status }: { status: TaskDto["status"] }) {
  const color =
    status === "running"
      ? "bg-primary animate-pulse"
      : status === "success"
        ? "bg-emerald-500"
        : status === "failed"
          ? "bg-destructive"
          : status === "cancelled"
            ? "bg-muted-foreground"
            : "bg-muted-foreground/50";
  return <span className={cn("h-2 w-2 shrink-0 rounded-full", color)} />;
}

function StatusBadge({ status }: { status: TaskDto["status"] }) {
  const map: Record<TaskDto["status"], string> = {
    pending: "等待中",
    running: "运行中",
    success: "成功",
    failed: "失败",
    cancelled: "已取消",
  };
  return (
    <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
      {map[status]}
    </span>
  );
}

function InfoView({ task }: { task: TaskDto }) {
  const rows: [string, string][] = [
    ["任务 ID", task.id],
    ["类型", task.taskType],
    ["名称", task.name],
    ["状态", task.status],
    ["退出码", task.exitCode === null ? "-" : String(task.exitCode)],
    ["开始", formatFull(task.createdAt)],
    ["结束", task.finishedAt === null ? "-" : formatFull(task.finishedAt)],
  ];
  return (
    <div className="rounded-lg border p-3">
      <dl className="grid grid-cols-[80px_1fr] gap-y-1.5 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{k}</dt>
            <dd className="break-all font-mono">{v}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

/** 输出区：运行中订阅事件缓冲；历史任务拉 DB 日志 */
function OutputView({
  task,
  running,
  liveLines,
  onError,
}: {
  task: TaskDto;
  running: boolean;
  liveLines: LiveLine[] | null;
  onError: (msg: string) => void;
}) {
  const [history, setHistory] = useState<LiveLine[] | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (running || liveLines !== null) return; // 事件缓冲优先
    let cancelled = false;
    void taskApi
      .logs(task.id)
      .then((rows) => {
        if (!cancelled) setHistory(rows.map((r) => ({ stream: r.stream, chunk: r.chunk })));
      })
      .catch((e) => !cancelled && onError(`加载日志失败: ${String(e)}`));
    return () => {
      cancelled = true;
    };
  }, [task.id, running, liveLines, onError]);

  const lines = liveLines ?? history ?? [];

  // 自动滚到底
  useEffect(() => {
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines.length]);

  return (
    <div
      ref={boxRef}
      data-testid="task-output"
      className="h-full overflow-auto rounded-lg border bg-black/80 p-3 font-mono text-sm leading-relaxed dark:bg-black/40"
    >
      {lines.length === 0 && (
        <span className="text-muted-foreground">{running ? "等待输出…" : "（无输出）"}</span>
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

function formatTime(unixSec: number): string {
  const d = new Date(unixSec * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return d.toDateString() === now.toDateString() ? hm : `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

function formatFull(unixSec: number): string {
  return new Date(unixSec * 1000).toLocaleString();
}

/** 秒级耗时格式化（结束时间缺省时按当前时间计算，用于运行中任务） */
function formatDuration(startSec: number, endSec: number | null): string {
  const ms = (endSec ?? Date.now() / 1000) - startSec;
  const s = Math.max(0, Math.round(ms));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}
