import { SubTabs } from "@/components/nav/SubTabs";
import { ForwardManager } from "@/features/adb/ForwardManager";
import { Placeholder } from "@/components/nav/Placeholder";

/**
 * ADB 页（原「终端」主 tab 改造）：多个子标签。
 * - 端口转发：五行默认行 + 可增行，每行独立建立/删除/验证；
 * - 终端等后续能力继续追加为子标签。
 */
export function AdbPage() {
  return (
    <SubTabs
      tabs={[
        {
          id: "forward",
          label: "端口转发",
          content: <ForwardManager />,
        },
        {
          id: "terminal",
          label: "终端",
          content: (
            <Placeholder
              title="多标签终端"
              description="命令执行、历史、输出过滤将基于任务系统（P2）在此实现"
              phase="后续阶段"
            />
          ),
        },
      ]}
    />
  );
}
