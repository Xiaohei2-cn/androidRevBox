import { forwardRef } from "react";
import { cn } from "@/lib/utils";

/**
 * 工具栏图标按钮（刷新/设置等卡内小动作）：统一 28px 热区、hover 填充、禁用态。
 * 旧版各页面散落的 `rounded p-1 hover:bg-accent` 小按钮全部收敛到这里，
 * 保证同一屏内工具按钮尺寸与反馈一致。
 */
export const IconButton = forwardRef<
  HTMLButtonElement,
  React.ButtonHTMLAttributes<HTMLButtonElement>
>(({ className, children, ...props }, ref) => (
  <button
    ref={ref}
    type="button"
    className={cn(
      "inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-40",
      className,
    )}
    {...props}
  >
    {children}
  </button>
));
IconButton.displayName = "IconButton";
