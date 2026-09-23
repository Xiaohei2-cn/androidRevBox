import { PageHeader } from "@/components/ui/page-header";
import { Placeholder } from "@/components/nav/Placeholder";
import { useI18n } from "@/i18n";

/** 工具市场（主 tab）：Frida / objection / jeb / radare2 等工具的一键安装与版本管理 */
export function MarketPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.market")} />
      <div className="min-h-0 flex-1">
        <Placeholder
          title="工具市场"
          description="Frida / objection / jeb / radare2 等工具的一键安装与版本管理将在此实现"
          phase="占位 · 规划中"
        />
      </div>
    </div>
  );
}
