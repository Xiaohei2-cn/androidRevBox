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
              // 子页是 keep-mounted（SubTabs 用 forceMount + hidden），所以两个页面都活着。
              // 访达拖放是 webview 级事件、不带坐标，只能靠这里告诉它"谁在眼前"：
              // 不传 active 的话，在托管页拖一个 .so 会同时填进 so 替换的输入框。
              content: <SoReplacePage active={subTab === "so-replace"} />,
            },
            {
              id: "hosting",
              label: "二进制托管",
              content: <BinaryHosting active={subTab === "hosting"} />,
            },
          ]}
        />
      </div>
    </div>
  );
}
