import { cn } from "@/lib/utils";

/**
 * 统一空态（UI 统一改版）：图标 + 标题 + 说明 + 上下文动作。
 * 规则：不只写「没有数据」，必须给出下一步动作（连接设备 / 重试 / 去配置）。
 * 占位页（Placeholder）与「未选择设备」「空目录」等复用同一语言。
 */
export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
}: {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex h-full min-h-[160px] flex-col items-center justify-center gap-2 rounded-2xl border border-dashed border-border px-6 py-10 text-center",
        className,
      )}
    >
      {icon && (
        <span className="mb-1 flex h-10 w-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
          {icon}
        </span>
      )}
      <p className="text-sm font-medium">{title}</p>
      {description && (
        <p className="max-w-md text-xs leading-relaxed text-muted-foreground">
          {description}
        </p>
      )}
      {action && <span className="mt-2">{action}</span>}
    </div>
  );
}
