import { cn } from "@/lib/utils";

/**
 * 页头（UI 统一改版）：每个内容页顶部的标题区，建立「我在哪个模块」的锚点。
 * 旧版所有页面内容直接顶到标题栏，页面之间没有呼吸感和身份区分。
 * 规则：标题 + 可选说明（一句话）+ 右侧至多一个主动作；
 * 子 tab 导航（SubTabs）自带页头时不再叠加。
 */
export function PageHeader({
  title,
  description,
  actions,
  className,
}: {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex shrink-0 items-center gap-3 pb-4 pt-1",
        className,
      )}
    >
      <div className="min-w-0 flex-1">
        <h1 className="truncate text-lg font-semibold leading-tight tracking-tight">
          {title}
        </h1>
        {description && (
          <p className="mt-1 truncate text-xs text-muted-foreground">
            {description}
          </p>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}
