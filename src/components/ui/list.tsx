import { cn } from "@/lib/utils";

/**
 * 全站列表语汇：**每一行是一个独立的气泡**，而不是被一条分割线切开的长条。
 *
 * 三层深度（与 shadcn/tauri-ui 的桌面刻度一致）：
 *   page-canvas → Card（bg-card + shadow-card）→ ListRow（bg-muted 凹槽 + 圆角 + 极淡外阴影）
 *
 * ## 布局契约（这条最容易被踩，写在这里）
 *
 * 行的高度与列对齐必须由 `ListRow` 自己保证，**不能指望调用方**：
 * 前几版把 `className` 直接加到 `<li>` 上，而 `li` 里还套着一层
 * `<button>`（可点行）。调用方写的 `flex items-center` 落在 `li` 上，
 * 但真正装子元素的是内层 `button`——于是子元素全按默认 `block` 排布，
 * 名字、权限胶囊、按钮各占一行或顶端对齐，看起来就是"元素没有竖向对齐"。
 *
 * 现在的做法：**布局类统一传给内层容器**，`li` 只负责外观（气泡、圆角、阴影）。
 * 内层容器固定 `flex items-center` + `min-height`，行高在单行与多行内容之间保持一致。
 */

/** 列表容器：行之间用 `gap` 留白，而不是分割线 */
export function List({
  className,
  testId,
  children,
}: {
  className?: string;
  testId?: string;
  children: React.ReactNode;
}) {
  return (
    <ul
      data-testid={testId}
      className={cn("flex min-w-0 flex-col gap-1.5 p-1.5", className)}
    >
      {children}
    </ul>
  );
}

/** 行布局的公共部分：竖向居中 + 统一最小高度（单行内容不会各行高矮不一） */
const ROW_LAYOUT = "flex w-full min-w-0 items-center gap-2";

/**
 * 单个气泡行。
 *
 * - `className` 里的**布局**类（flex/grid/gap/px/py）作用于内层内容容器；
 * - 外观类（bg/border/shadow）留在 `li` 上，作为气泡本体；
 * - 需要多行内容时传 `multiline`（改用 `items-start`），避免多行文字被强行垂直居中。
 */
export function ListRow({
  className,
  testId,
  interactive = true,
  selected = false,
  disabled = false,
  multiline = false,
  onClick,
  onDoubleClick,
  title,
  children,
}: {
  className?: string;
  testId?: string;
  interactive?: boolean;
  selected?: boolean;
  disabled?: boolean;
  /** 行内容需要多行（备注、参数）时用顶部对齐，而不是垂直居中 */
  multiline?: boolean;
  onClick?: () => void;
  onDoubleClick?: () => void;
  title?: string;
  children: React.ReactNode;
}) {
  const shell = cn(
    "group relative min-w-0 rounded-xl border shadow-row transition-colors",
    "bg-muted/60 dark:bg-muted/35",
    disabled && "opacity-60",
    selected
      ? "border-primary/50 bg-primary/[0.08] ring-1 ring-primary/20 dark:bg-primary/[0.12]"
      : interactive && "border-transparent hover:border-border hover:bg-muted/80 dark:hover:bg-muted/45",
    !selected && !interactive && "border-transparent",
  );

  // 调用方给的 className 里通常混着"布局"和"外观"；这里整体交给内容容器，
  // 外观类（bg-*/border-*/shadow-*）重复写在容器上也无害——气泡本体已经画在 li 上。
  const content = cn(
    ROW_LAYOUT,
    "px-2.5 py-2",
    multiline ? "items-start" : "items-center",
    className,
  );

  if (!onClick && !onDoubleClick) {
    return (
      <li data-testid={testId} title={title} data-selected={selected ? "true" : undefined} className={shell}>
        <div className={content}>{children}</div>
      </li>
    );
  }
  return (
    <li data-testid={testId} title={title} data-selected={selected ? "true" : undefined} className={shell}>
      <button
        type="button"
        className={cn(
          content,
          "rounded-xl text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          disabled ? "cursor-default" : "cursor-pointer",
        )}
        disabled={disabled}
        onClick={onClick}
        onDoubleClick={onDoubleClick}
      >
        {children}
      </button>
    </li>
  );
}

/**
 * 表头：列宽与 `ListRow` 的子元素一一对应。
 * 左右内边距与行内 `px-2.5` 对齐，否则表头文字会比数据列偏左 2px——那就是"没对齐"。
 */
export function ListHeader({
  className,
  testId,
  children,
}: {
  className?: string;
  testId?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      data-testid={testId}
      className={cn(
        // px-2.5 = List 的 p-1.5 + 行内 px-2.5：表头的每一列必须压在数据列的同一 x 上，
        // 少这一层容器内边距，表头就会整体左移 6px，看起来像"表头没对齐"
        "flex items-center gap-2 px-4 pb-1.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** 空状态：列表为空时的一句话，居中且留足高度，别让卡片塌成一条线 */
export function ListEmpty({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <p
      className={cn(
        "rounded-lg border border-dashed border-border/60 px-3 py-6 text-center text-[11px] text-muted-foreground",
        className,
      )}
    >
      {children}
    </p>
  );
}
