import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { HookToolsPanel } from "@/features/market/HookToolsPanel";
import { useI18n } from "@/i18n";

/**
 * 插件页（P13）：工具市场的入口。
 *
 * 第一个子 tab 是「hook 工具」——按能力分组（这里放的都是调 hook / 追踪的现成工具），
 * 不按"插件来源"分组：用户找的是"能不能帮我 hook 这个进程"，不是"这个插件是谁写的"。
 * 其余分组留给后续的工具，先给空态，不假装有货。
 */
export function PluginsPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.plugins")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          tabs={[
            {
              id: "hook-tools",
              label: t("tools.tab"),
              content: <HookToolsPanel />,
            },
            {
              id: "more",
              label: t("common.phasePlaceholder"),
              content: (
                <Placeholder
                  title={t("nav.plugins")}
                  description={t("tools.listHint")}
                  phase={t("common.phasePlaceholder")}
                />
              ),
            },
          ]}
        />
      </div>
    </div>
  );
}
