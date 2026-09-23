import { PageHeader } from "@/components/ui/page-header";
import { Placeholder } from "@/components/nav/Placeholder";
import { useI18n } from "@/i18n";

/** 插件中心：完整插件生命周期管理在 P6 阶段实现 */
export function PluginsPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.plugins")} />
      <div className="min-h-0 flex-1">
        <Placeholder
          title="插件中心"
          description="插件安装、启停、升级、回滚、崩溃隔离将在此实现"
          phase="P6 · 插件中心"
        />
      </div>
    </div>
  );
}
