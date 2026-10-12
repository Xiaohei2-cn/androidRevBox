import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { IconTile } from "./icon-tile";

/**
 * 面板头（全站统一）：图标瓦 + 标题 +（可选）一句话说明 +（可选）右侧动作。
 *
 * 之前只有仪表盘/设置那几张 `Card` 有正式标题行，工具类面板（进程端口、托管二进制、
 * frida 脚本/设置、终端）多是裸 `<h3>` 或干脆没标题——同一屏里两种层级语言，
 * 看着就像"没做完"。算法中心定下 PaneHeader 之后，全站按同一行排版。
 */
export function PaneHeader({
  icon: Icon,
  toneClass = "bg-primary/10 text-primary",
  title,
  description,
  actions,
  className,
  compact = false,
}: {
  icon?: LucideIcon;
  toneClass?: string;
  title: string;
  description?: string;
  actions?: React.ReactNode;
  className?: string;
  /** 紧凑档：窄面板与侧栏用（图标瓦也一起缩小） */
  compact?: boolean;
}) {
  return (
    <header className={cn("flex min-w-0 shrink-0 items-center gap-2", className)}>
      {Icon && <IconTile icon={Icon} toneClass={toneClass} size={compact ? "sm" : "md"} />}
      <div className="min-w-0 flex-1">
        <h3 className="min-w-0 truncate text-xs font-semibold leading-tight">{title}</h3>
        {description && (
          <p className="mt-0.5 min-w-0 truncate text-[11px] leading-tight text-muted-foreground">
            {description}
          </p>
        )}
      </div>
      {actions && (
        <div className="flex shrink-0 items-center gap-1.5">{actions}</div>
      )}
    </header>
  );
}
