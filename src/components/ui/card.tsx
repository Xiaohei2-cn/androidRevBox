import { forwardRef } from "react";
import { cn } from "@/lib/utils";

/**
 * 统一卡片（UI 统一改版）：全站唯一的卡片语言。
 * - 圆角 rounded-2xl（16px，落在 12–24px 桌面区间）、shadow-card 浮起、bg-card 表面。
 * - 页面背景用 .page-canvas 压暗一档，卡片靠「略深底 + 浅边框 + 阴影」三层分离，
 *   不再依赖纯 1px 边框（旧版所有面同色，层级扁平）。
 */
export const Card = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        "rounded-2xl border border-border/70 bg-card shadow-card",
        className,
      )}
      {...props}
    />
  ),
);
Card.displayName = "Card";

/** 卡片标题行：图标 + 标题 +（右侧动作区） */
export const CardHeader = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn("flex items-center gap-2 px-4 pt-4", className)}
      {...props}
    />
  ),
);
CardHeader.displayName = "CardHeader";

export const CardTitle = forwardRef<
  HTMLHeadingElement,
  React.HTMLAttributes<HTMLHeadingElement>
>(({ className, ...props }, ref) => (
  <h3
    ref={ref}
    className={cn("min-w-0 truncate text-sm font-semibold leading-none", className)}
    {...props}
  />
));
CardTitle.displayName = "CardTitle";

/** 卡片正文区：与标题行之间留 8px（pt-2），底部留 16px */
export const CardContent = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div ref={ref} className={cn("px-4 pb-4 pt-2", className)} {...props} />
  ),
);
CardContent.displayName = "CardContent";
