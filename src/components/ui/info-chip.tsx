/** 端口/属主一类的小 chip：设备页与 ADB 侧几个 tab 共用，不放进任何一边的私有文件里 */
import { cn } from "@/lib/utils";

export function InfoChip({
  label,
  testid,
  title,
  className,
}: {
  label: string;
  testid: string;
  title?: string;
  className?: string;
}) {
  return (
    <span
      data-testid={testid}
      title={title}
      className={cn(
        "path-selectable max-w-full break-all rounded bg-emerald-500/10 px-1.5 py-0.5 font-mono text-emerald-500",
        className,
      )}
    >
      {label}
    </span>
  );
}
