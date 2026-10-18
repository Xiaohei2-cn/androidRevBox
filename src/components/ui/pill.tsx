import { cn } from "@/lib/utils";

/**
 * 属性胶囊：把"值"从"动作"里区分出来的唯一底板。
 *
 * 为什么不用 Badge：Badge 是**带语义色**的标签（系统应用/用户应用、状态），
 * 而权限位、端口、UID 这类是**属性值**——它们不该有绿/黄/红的暗示。
 * 属性用一个中性的、内凹的胶囊底板，读作"这是这一行的某个字段值"。
 *
 * 形状取 `rounded-full`（真椭圆），不是小圆角方块：
 * 表格行里已经有圆角按钮，方块状的值会与按钮抢"可点"的暗示；
 * 全圆角在视觉上离按钮最远，一眼就是"只读的标签"。
 *
 * 内阴影（`shadow-pill`）而不是外阴影：外阴影会浮起来像按钮，
 * 内阴影才是"刻进位里的凹槽"。深色主题下内阴影不可见，改用亮色描边补同样的凹陷感。
 */
export type PillTone = "neutral" | "mono" | "accent" | "muted";

const TONE_CLASS: Record<PillTone, string> = {
  /** 默认：中性属性值（权限、UID、路径片段） */
  neutral:
    "bg-muted/70 text-foreground shadow-pill dark:bg-muted/50 dark:shadow-none dark:ring-1 dark:ring-inset dark:ring-white/[0.06]",
  /** 等宽数字/十六进制（权限位、端口、地址）：tabular-nums 保证逐行对齐 */
  mono:
    "bg-muted/70 font-mono tabular-nums text-foreground shadow-pill dark:bg-muted/50 dark:shadow-none dark:ring-1 dark:ring-inset dark:ring-white/[0.06]",
  /** 强调值（当前选中的端口、匹配到的项）：主色淡底，仍然保持只读观感 */
  accent:
    "bg-primary/10 font-mono tabular-nums text-primary shadow-pill dark:bg-primary/15 dark:shadow-none",
  /** 弱化值（不可用、空值、次要属性） */
  muted:
    "bg-muted/40 text-muted-foreground shadow-pill dark:bg-muted/30 dark:shadow-none",
};

export function Pill({
  tone = "neutral",
  title,
  testId,
  className,
  children,
}: {
  tone?: PillTone;
  title?: string;
  testId?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      data-testid={testId}
      title={title}
      className={cn(
        // px-2.5 是椭圆端头的最小可用宽度：px-2 时 `-rwxr-xr-x` 这种 10 字符串
        // 会被两端圆弧切掉一点，读起来像被压扁；py 不做（高由行高决定）
        "inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-2.5",
        "text-[10px] font-medium leading-none",
        TONE_CLASS[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

/**
 * 权限位胶囊：`-rwxr-xr-x` 这种字符串单独一格。
 *
 * 为什么单独抽一个：权限是全站最容易"每处写法都不一样"的字段
 * （设备页、托管页各写各的等宽颜色）。这里固定四件事：
 * ① 等宽 + `tabular-nums`，逐行读时三个权限组必须对齐；
 * ② 首字符（文件类型 `-`/`d`/`l`）与权限段**分色**——类型是另一类信息；
 * ③ 空值不占位（`"-"`），而不是渲染一个空胶囊；
 * ④ 完整权限串放 `title`，长编号设备上也不靠截断猜。
 */
export function PermissionPill({
  perms,
  testId,
  className,
}: {
  perms: string | null | undefined;
  testId?: string;
  className?: string;
}) {
  if (!perms) {
    return (
      <span className={cn("font-mono text-[10px] text-muted-foreground/60", className)}>
        -
      </span>
    );
  }
  const kind = perms.slice(0, 1);
  const bits = perms.slice(1);
  return (
    <Pill tone="mono" title={perms} testId={testId} className={className}>
      <span className="text-muted-foreground/70">{kind}</span>
      {/* 不加 tracking-tight：`rwxr-xr-x` 在等宽字体里本来就对齐，
          收紧字距反而让它与相邻列的数字挤在一起 */}
      <span>{bits}</span>
    </Pill>
  );
}
