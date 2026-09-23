import { PageHeader } from "@/components/ui/page-header";
import { Placeholder } from "@/components/nav/Placeholder";
import { useI18n } from "@/i18n";

/** 内核页占位：内核镜像/模块分析将在后续阶段实现 */
export function KernelPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.kernel")} />
      <div className="min-h-0 flex-1">
        <Placeholder
          title="内核"
          description="boot.img 解包、内核版本/配置探测、模块列表将在此实现"
          phase="占位 · 规划中"
        />
      </div>
    </div>
  );
}
