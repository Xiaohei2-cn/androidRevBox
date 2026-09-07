interface PlaceholderProps {
  title: string;
  description: string;
  phase: string;
}

/** P0 占位内容：向用户明示该区域将在哪个阶段实现 */
export function Placeholder({ title, description, phase }: PlaceholderProps) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="max-w-md rounded-xl border border-dashed p-8 text-center">
        <p className="text-sm font-medium">{title}</p>
        <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
          {description}
        </p>
        <span className="mt-4 inline-block rounded-full bg-muted px-2.5 py-0.5 text-xs text-muted-foreground">
          {phase}
        </span>
      </div>
    </div>
  );
}
