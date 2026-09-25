import { useEffect, useState } from "react";
import { SubTabs } from "@/components/nav/SubTabs";
import { PageHeader } from "@/components/ui/page-header";
import { BinaryHosting } from "@/features/binary/BinaryHosting";
import { SoReplacePage } from "@/features/binary/SoReplacePage";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";

/**
 * 二进制页：对设备上二进制的两件事。
 * - so 替换：把修补后的 .so 写回安装目录（免重打包，AR8.4）；
 * - 二进制托管：/data/local/tmp 下 ELF 的浏览/授权/后台执行/终止（第四十七轮从 ADB 页挪来）。
 * frida 前置检查的「去托管 frida-server」深链指向这里（pendingBinaryTab）。
 */
export function BinaryPage() {
  const { t } = useI18n();
  const { pendingBinaryTab } = useAppNav();
  const [subTab, setSubTab] = useState("so-replace");
  useEffect(() => {
    if (pendingBinaryTab) setSubTab(pendingBinaryTab);
  }, [pendingBinaryTab]);
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.binary")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          value={subTab}
          onValueChange={setSubTab}
          tabs={[
            {
              id: "so-replace",
              label: "so 替换",
              content: <SoReplacePage />,
            },
            {
              id: "hosting",
              label: "二进制托管",
              content: <BinaryHosting />,
            },
          ]}
        />
      </div>
    </div>
  );
}
