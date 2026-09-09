import { useCallback, type MouseEvent } from "react";
import { cn } from "@/lib/utils";

/**
 * 可复制路径文本（P7 用户反馈）：
 * - 全局 body 是 user-select:none，这里开 user-select:all 让路径可拖选/右键复制；
 * - 双击一次即全选整条（配合 select 事件给个短暂「已复制提示」由调用方决定）；
 * - title 悬浮显示完整值，单行省略。
 * 空值传 null 显示占位破折号且不可选。
 */
export function PathText({
  value,
  className,
  testid,
  placeholder = "—",
}: {
  value?: string | null;
  className?: string;
  testid?: string;
  placeholder?: string;
}) {
  const selectAll = useCallback((e: MouseEvent<HTMLElement>) => {
    const el = e.currentTarget;
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
  }, []);

  if (!value) {
    return (
      <span data-testid={testid} className={cn("text-muted-foreground", className)}>
        {placeholder}
      </span>
    );
  }

  return (
    <span
      data-testid={testid}
      title={value}
      onDoubleClick={selectAll}
      className={cn(
        "path-selectable inline-block max-w-full truncate align-bottom",
        className,
      )}
    >
      {value}
    </span>
  );
}
