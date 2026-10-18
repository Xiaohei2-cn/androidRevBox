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

/**
 * 卡片标题行：图标 + 标题 +（右侧动作区）。
 *
 * `min-h-11` 而不是固定高度：标题行要与表格行、按钮行共用一条基线。
 * 同一张卡里"标题 24px、行 28px、按钮 32px"三种高度轮流出现，看着就是没对齐。
 * 底部那条分隔线不在这里写死——纯指标卡不需要，带正文的卡由调用方加 `border-b`。
 */
export const CardHeader = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn("flex min-h-11 items-center gap-2 px-4 pt-3.5", className)}
      {...props}
    />
  ),
);
CardHeader.displayName = "CardHeader";

/**
 * 卡片标题。字号 `text-sm`（本仓库刻度 = 16px）与 `font-semibold` 是全站标题的唯一组合：
 * 层级的差别靠**内容层级**（标题 / 副标题 / 正文）而不是靠每处自己调字号。
 */
export const CardTitle = forwardRef<
  HTMLHeadingElement,
  React.HTMLAttributes<HTMLHeadingElement>
>(({ className, ...props }, ref) => (
  <h3
    ref={ref}
    className={cn("min-w-0 truncate text-sm font-semibold leading-tight tracking-[-0.01em]", className)}
    {...props}
  />
));
CardTitle.displayName = "CardTitle";

/**
 * 卡片正文区。左右 4（16px）与 CardHeader 完全一致——两者不同宽是"卡片内部不对齐"
 * 最常见的样子；上下留白沿用同一刻度，卡片不再各自漂移。
 */
export const CardContent = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div ref={ref} className={cn("px-4 pb-4 pt-2.5", className)} {...props} />
  ),
);
CardContent.displayName = "CardContent";
/** 卡片副标题：一句话说明这块面板是干什么的（标题只放名词） */
export const CardSubtitle = forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, ...props }, ref) => (
  <p
    ref={ref}
    className={cn("min-w-0 truncate text-[11px] leading-none text-muted-foreground", className)}
    {...props}
  />
));
CardSubtitle.displayName = "CardSubtitle";

/**
 * 卡片底栏：放"存取/清除"这类次要动作，与顶部的主动作分层。
 * 左右内边距与表头/正文一致（`px-4`），只把上下收紧——底栏是动作区，不需要正文的留白。
 */
export const CardFooter = forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        "flex shrink-0 flex-wrap items-center gap-1.5 border-t border-border/60 px-4 py-2",
        className,
      )}
      {...props}
    />
  ),
);
CardFooter.displayName = "CardFooter";
