import { cn } from "@/lib/utils";

/**
 * 小徽章（对应 shadcn 的 Badge，取本仓库既有的 tone 口径）。
 * 与 StatusBadge 的分工：StatusBadge 表达"状态"（ok/warn/error/info），
 * Badge 表达"属性/计数/参数值"这类中性信息，不该带状态色。
 */
export type BadgeVariant = "muted" | "outline" | "primary" | "mono";

/**
 * 形状统一为真椭圆（`rounded-full`）——与 Pill / StatusBadge 一致。
 * 原来是小圆角方块：表格行里已经有圆角按钮，方块状的徽章会与按钮抢"可点"的暗示。
 */
const VARIANT_CLASS: Record<BadgeVariant, string> = {
  muted: "bg-muted text-muted-foreground",
  outline: "border border-border/70 text-muted-foreground",
  primary: "bg-primary/10 text-primary",
  mono: "bg-muted/70 font-mono tabular-nums text-muted-foreground",
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
        "inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-2 text-[10px] font-medium leading-none",
        VARIANT_CLASS[variant],
        className,
      )}
    >
      {children}
    </span>
  );
}
