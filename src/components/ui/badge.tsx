import { cn } from "@/lib/utils";

/**
 * 小徽章（对应 shadcn 的 Badge，取本仓库既有的 tone 口径）。
 * 与 StatusBadge 的分工：StatusBadge 表达"状态"（ok/warn/error/info），
 * Badge 表达"属性/计数/参数值"这类中性信息，不该带状态色。
 */
export type BadgeVariant = "muted" | "outline" | "primary" | "mono";

const VARIANT_CLASS: Record<BadgeVariant, string> = {
  muted: "bg-muted text-muted-foreground",
  outline: "border border-border/70 text-muted-foreground",
  primary: "bg-primary/10 text-primary",
  mono: "bg-muted/70 font-mono text-muted-foreground",
};

export function Badge({
  variant = "muted",
  className,
  title,
  children,
}: {
  variant?: BadgeVariant;
  className?: string;
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-medium leading-4",
        VARIANT_CLASS[variant],
        className,
      )}
    >
      {children}
    </span>
  );
}
