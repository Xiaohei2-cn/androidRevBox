import { forwardRef } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

export type SelectProps = React.SelectHTMLAttributes<HTMLSelectElement> & {
  /** 外层容器的类：宽度、shrink、对齐这些放这里（原生 select 自己撑不出右侧箭头区） */
  containerClassName?: string;
};

/**
 * 统一的下拉控件。
 *
 * 换掉裸 `<select>` 的原因不是"好看一点"：裸控件在本仓库里有三处不一致——
 * 高度有 h-6/h-7/h-8 三种、圆角有 rounded/rounded-md 两种、底色一律 bg-transparent
 * （在半透明面板上看着像没描边），而且各站点是自己手写箭头都没有。
 * 这里定一套：`appearance-none` + 内建箭头、与 button/input 同刻度圆角、
 * 底色走 bg-background/70（在凹槽与卡片上都能读出"这是个可点的字段"）。
 *
 * 箭头用 `inset-y-0 + flex` 居中，**不用 -translate-y-1/2**：
 * transform 会在透明窗口里给元素单独立合成层（globals.css 第三轮闪烁修复）。
 */
export const Select = forwardRef<HTMLSelectElement, SelectProps>(
  ({ className, containerClassName, children, ...props }, ref) => (
    <span
      className={cn(
        "relative inline-flex min-w-0 shrink-0 items-center",
        containerClassName,
      )}
    >
      <select
        ref={ref}
        className={cn(
          "h-8 min-w-0 max-w-full appearance-none rounded-lg border border-input bg-background/70",
          "pl-2 pr-6 text-xs shadow-sm transition-colors",
          "hover:border-primary/40",
          "focus-visible:border-primary/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
          "disabled:cursor-not-allowed disabled:opacity-50",
          className,
        )}
        {...props}
      >
        {children}
      </select>
      <span className="pointer-events-none absolute inset-y-0 right-1.5 flex items-center text-muted-foreground">
        <ChevronDown className="h-3 w-3" />
      </span>
    </span>
  ),
);
Select.displayName = "Select";
