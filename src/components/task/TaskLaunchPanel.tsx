import { useCallback, useRef, useState } from "react";
import { LoaderCircle, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TaskSessionView } from "./TaskSessionView";

/**
 * 长操作发起面板：输入 → onRun 返回 task_id → 挂 TaskSessionView 看实时输出/取消。
 * 设备页 shell/logcat 共用；父级负责校验输入并在非法时返回 null。
 */
export function TaskLaunchPanel({
  label,
  placeholder,
  disabled,
  onRun,
}: {
  label: string;
  placeholder: string;
  onRun: (input: string) => Promise<string | null>;
  disabled?: boolean;
}) {
  const [input, setInput] = useState("");
  const [taskId, setTaskId] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);

  const launch = useCallback(
    async (value: string) => {
      if (busy.current) return;
      busy.current = true;
      setPending(true);
      setError(null);
      try {
        const id = await onRun(value);
        if (id) setTaskId(id);
        else setError("输入无效");
      } catch (e) {
        setError(String((e as { message?: string }).message ?? e));
      } finally {
        setPending(false);
        busy.current = false;
      }
    },
    [onRun],
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 items-center gap-2">
        <span className="text-xs font-medium">{label}</span>
        <input
          value={input}
          disabled={disabled}
          placeholder={placeholder}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !disabled) void launch(input);
          }}
          className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
        />
        <Button size="sm" disabled={disabled || pending} onClick={() => void launch(input)}>
          {pending ? (
            <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Play className="h-3.5 w-3.5" />
          )}
          运行
        </Button>
      </div>
      {error && <p className="shrink-0 text-xs text-destructive">{error}</p>}
      <div className="min-h-0 flex-1">
        {taskId ? (
          <TaskSessionView key={taskId} taskId={taskId} />
        ) : (
          <div className="flex h-full min-h-[120px] items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground">
            {disabled ? "当前不可用" : "运行后此处显示实时输出"}
          </div>
        )}
      </div>
    </div>
  );
}
