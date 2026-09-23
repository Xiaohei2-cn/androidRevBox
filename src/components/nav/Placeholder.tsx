import { EmptyState } from "@/components/ui/empty-state";

interface PlaceholderProps {
  title: string;
  description: string;
  phase: string;
}

/**
 * P0 占位内容：向用户明示该区域将在哪个阶段实现。
 * UI 统一改版：复用 EmptyState 的空态语言（虚线框 + 居中 + 阶段 chip），
 * 不再是一个素色小方块挂在空白里。
 */
export function Placeholder({ title, description, phase }: PlaceholderProps) {
  return (
    <EmptyState
      title={title}
      description={description}
      action={
        <span className="rounded-full bg-muted px-2.5 py-0.5 text-xs text-muted-foreground">
          {phase}
        </span>
      }
    />
  );
}
