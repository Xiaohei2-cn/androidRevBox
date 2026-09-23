import { cn } from "@/lib/utils";

/**
 * 统一状态徽章：全站状态色的唯一出口。
 * tone 语义固定——ok=绿（就绪/成功）、warn=琥珀（未配置/未检测到，常态非错误）、
 * error=红（异常/失败）、info=天蓝（进行中）、muted=灰（中性/停用）。
 * 同一数据在列表与详情里必须用同一 tone，不允许装饰性乱用。
 */
export type BadgeTone = "ok" | "warn" | "error" | "info" | "muted";

const TONE_CLASS: Record<BadgeTone, string> = {
  ok: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
  warn: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  error: "bg-red-500/15 text-red-600 dark:text-red-400",
  info: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
  muted: "bg-muted text-muted-foreground",
};

export function StatusBadge({
  tone,
  className,
  children,
}: {
  tone: BadgeTone;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-xs font-medium leading-4",
        TONE_CLASS[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}
