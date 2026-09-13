import { SubTabs } from "@/components/nav/SubTabs";
import { ForwardManager } from "@/features/adb/ForwardManager";
import { BinaryHosting } from "@/features/adb/BinaryHosting";
import { ProcPorts } from "@/features/adb/ProcPorts";
import { Placeholder } from "@/components/nav/Placeholder";

/**
 * ADB 页（原「终端」主 tab 改造）：多个子标签。
 * - 端口转发：五行默认行 + 可增行，每行独立建立/删除/验证；
 * - 二进制托管：/data/local/tmp 下 ELF 的浏览/授权/后台执行/终止；
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
          id: "binary-hosting",
          label: "二进制托管",
          content: <BinaryHosting />,
        },
        {
          id: "proc-ports",
          label: "进程端口",
          content: <ProcPorts />,
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
