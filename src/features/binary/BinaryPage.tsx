import { SubTabs } from "@/components/nav/SubTabs";
import { PageHeader } from "@/components/ui/page-header";
import { SoReplacePage } from "@/features/binary/SoReplacePage";
import { useI18n } from "@/i18n";

/** 二进制页：so 替换（修补后的 .so 写回安装目录，免重打包） */
export function BinaryPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.binary")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          tabs={[
            {
              id: "so-replace",
              label: "so 替换",
              content: <SoReplacePage />,
            },
          ]}
        />
      </div>
    </div>
  );
}
