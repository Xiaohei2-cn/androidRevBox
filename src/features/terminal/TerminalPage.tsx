import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";

/** 终端页：多标签终端在 P7 UX 阶段基于 P2 任务系统实现 */
export function TerminalPage() {
  return (
    <SubTabs
      tabs={[
        {
          id: "term-1",
          label: "终端 1",
          content: (
            <Placeholder
              title="多标签终端"
              description="命令执行、历史、输出过滤将基于任务系统（P2）在此实现"
              phase="P7 · UX 完善"
            />
          ),
        },
      ]}
    />
  );
}
