import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { FridaPage } from "@/features/hook/frida/FridaPage";
import { useI18n } from "@/i18n";

/**
 * Hook 主标签（P10）：子标签化——frida 会话工作台 + 其余能力后续追加。
 */
export function HookPage() {
  const { t } = useI18n();
  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.hook")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          tabs={[
            { id: "frida", label: t("hook.frida.tab"), content: <FridaPage /> },
            {
              id: "more",
              label: t("hook.more.tab"),
              content: (
                <Placeholder
                  title={t("hook.more.title")}
                  description={t("hook.more.desc")}
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
