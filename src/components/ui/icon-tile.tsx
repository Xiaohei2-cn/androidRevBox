import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * 图标瓦：面板头与列表分组头的统一视觉锚点。
 * 本仓库的卡片头原来只有一枚 Lucide 线稿图标 + 文字，密集界面里"找面板"全靠读字；
 * 加一层有色底块后，扫一眼就能定位到清单/配方/输入/输出四块。
 */
export function IconTile({
  icon: Icon,
  toneClass = "bg-primary/10 text-primary",
  className,
  size = "md",
}: {
  icon: LucideIcon;
  /** 背景/前景一类色（用 tailwind 语义色，与 StatusBadge 同一写法） */
  toneClass?: string;
  className?: string;
  size?: "sm" | "md";
}) {
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-lg",
        size === "sm" ? "h-5 w-5" : "h-7 w-7",
        toneClass,
        className,
      )}
    >
      <Icon className={size === "sm" ? "h-3 w-3" : "h-3.5 w-3.5"} />
    </span>
  );
}
