import { forwardRef } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * 按钮（对齐 tauri-ui / shadcn 的桌面刻度）。
 *
 * 三件事在这里定死，别的页面不再各自调：
 * ① **高度档位**：sm 28 / md 32 / default 36 / lg 40，与表格行（28）、卡片标题行（44）对得上；
 * ② **圆角随尺寸走**：小按钮 `rounded-md`（20px 见方上用大圆角会糊成一粒），
 *    常规则 `rounded-lg`——与输入框、下拉同一个刻度；
 * ③ **焦点环 `ring-2` + offset**：键盘 Tab 时按钮必须一眼认出来，
 *    透明窗口下不能靠背景色微变来暗示。
 *
 * `ghost` 与 `outline` 的分工保持不变：ghost = 行内图标动作（悬停才给底），
 * outline = 卡片里成组的次要动作（始终有边界，用户知道它可点）。
 *
 * 注意：本文件被 `chefUiGuardrails.test.tsx` 逐字扫描，不许出现 transform/动画类写法。
 */
const buttonVariants = cva(
  [
    "inline-flex select-none items-center justify-center gap-1.5 whitespace-nowrap font-medium",
    "transition-colors duration-150",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
    "disabled:pointer-events-none disabled:opacity-45",
    "[&_svg]:pointer-events-none [&_svg]:shrink-0",
  ].join(" "),
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground shadow-control hover:bg-primary/90",
        secondary: "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        outline:
          "border border-input bg-background/60 shadow-control hover:border-primary/40 hover:bg-accent hover:text-accent-foreground",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        destructive:
          "bg-destructive text-destructive-foreground shadow-control hover:bg-destructive/90",
        /** 链接式：只用于"去别处"的文字入口，不带任何底板 */
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        /** 28px：表格行、紧凑工具条（与行内文字同一基线） */
        sm: "h-7 rounded-md px-2.5 text-[11px]",
        /** 32px：卡片底栏、次级工具条 */
        md: "h-8 rounded-lg px-3 text-xs",
        /** 36px：表单与卡片的主动作 */
        default: "h-9 rounded-lg px-4 text-xs",
        lg: "h-10 rounded-xl px-6 text-sm",
        /** 图标按钮：边长与高度同档，并排时不歪 */
        icon: "h-9 w-9 rounded-lg",
        "icon-sm": "h-7 w-7 rounded-md",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, ...props }, ref) => (
    <button ref={ref} className={cn(buttonVariants({ variant, size, className }))} {...props} />
  ),
);
Button.displayName = "Button";

export { buttonVariants };
