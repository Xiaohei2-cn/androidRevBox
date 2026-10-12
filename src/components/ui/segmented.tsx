import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * 分段开关（Radix Tabs 的视觉语言，但不带 tab 语义）。
 * 用它的理由：本仓库里"互斥的少数几个模式"原来是 `<select>`（输入口径、输出口径）
 * 或"自动执行：开"这种读数按钮（自动/手动）。select 在桌面端要点两下、读数按钮要读一句
 * 中文才知道当前状态；分段开关把状态画在选中段上，一眼可见、一次点击。
 *
 * 注意：**不用 transform/transition-all 做滑块动画**。这是透明窗口，transform 会让
 * WebKit 给元素单独立合成层，层创建/销毁那一帧以满 alpha 打底 = 整窗闪一下
 * （见 globals.css 第三轮闪烁修复与 docs/algorithm_center.md）。
 */
export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  icon?: LucideIcon;
  title?: string;
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  ariaLabel,
  className,
  compact = false,
}: {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (next: T) => void;
  ariaLabel: string;
  className?: string;
  /** 紧凑档：面板头里用，高度与 h-7 控件对齐 */
  compact?: boolean;
}) {
  return (
    <span
      role="group"
      aria-label={ariaLabel}
      className={cn(
        "inline-flex shrink-0 items-center gap-0.5 rounded-lg bg-muted p-0.5",
        compact ? "h-7" : "h-8",
        className,
      )}
    >
      {options.map((option) => {
        const active = option.value === value;
        const Icon = option.icon;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            title={option.title ?? option.label}
            onClick={() => onChange(option.value)}
            className={cn(
              "inline-flex h-full items-center gap-1 rounded-md px-2 text-[11px] font-medium",
              "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              active
                ? "bg-card text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {Icon && <Icon className="h-3 w-3" />}
            <span className="truncate">{option.label}</span>
          </button>
        );
      })}
    </span>
  );
}
