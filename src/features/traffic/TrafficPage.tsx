import { PageHeader } from "@/components/ui/page-header";
import { Placeholder } from "@/components/nav/Placeholder";
import { useI18n } from "@/i18n";

/** 流量页占位：实时网速、按应用流量统计与抓包入口将在后续阶段实现 */
export function TrafficPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.traffic")} />
      <div className="min-h-0 flex-1">
        <Placeholder
          title="流量"
          description="设备实时网速、按应用流量统计与抓包入口将在此实现"
          phase="占位 · 规划中"
        />
      </div>
    </div>
  );
}
