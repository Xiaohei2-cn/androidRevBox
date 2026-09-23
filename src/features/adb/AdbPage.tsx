import { useEffect, useState } from "react";
import { SubTabs } from "@/components/nav/SubTabs";
import { PageHeader } from "@/components/ui/page-header";
import { ForwardManager } from "@/features/adb/ForwardManager";
import { BinaryHosting } from "@/features/adb/BinaryHosting";
import { ProcPorts } from "@/features/adb/ProcPorts";
import { Placeholder } from "@/components/nav/Placeholder";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";

/**
 * ADB 页（原「终端」主 tab 改造）：多个子标签。
 * - 端口转发：五行默认行 + 可增行，每行独立建立/删除/验证；
 * - 二进制托管：/data/local/tmp 下 ELF 的浏览/授权/后台执行/终止；
 * - 终端等后续能力继续追加为子标签。
 * P10：frida 前置检查失败时经 pendingAdbTab 深链切到转发/托管 tab。
 */
export function AdbPage() {
  const { pendingAdbTab } = useAppNav();
  const { t } = useI18n();
  const [subTab, setSubTab] = useState("forward");
  useEffect(() => {
    if (pendingAdbTab) setSubTab(pendingAdbTab);
  }, [pendingAdbTab]);
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.terminal")} />
      <div className="min-h-0 flex-1">
    <SubTabs
      value={subTab}
      onValueChange={setSubTab}
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
      </div>
    </div>
  );
}
