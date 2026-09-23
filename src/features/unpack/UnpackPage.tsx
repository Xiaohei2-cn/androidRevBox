import { PageHeader } from "@/components/ui/page-header";
import { Placeholder } from "@/components/nav/Placeholder";
import { useI18n } from "@/i18n";

/** 脱壳页占位：壳识别、dump、修复将在后续阶段实现 */
export function UnpackPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.unpack")} />
      <div className="min-h-0 flex-1">
        <Placeholder
          title="脱壳"
          description="加固壳识别、内存 dump、dex 修复与重组将在此实现"
          phase="占位 · 规划中"
        />
      </div>
    </div>
  );
}
