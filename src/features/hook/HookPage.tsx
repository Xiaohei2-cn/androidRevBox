import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { FridaPage } from "@/features/hook/frida/FridaPage";
import { useI18n } from "@/i18n";

/**
 * Hook 主标签（P10）：子标签化——frida 会话工作台 + 其余能力后续追加。
 *
 * 第八十四轮按用户纠正**撤掉了这里单独挂的「真终端」子标签**：
 * 真终端不是一个新地方，它就是 frida 会话右侧那一整块控制台的实现（在 `frida/FridaPage` 里）。
 * 之前在这里另开一块屏，等于把"把厨房灶台换成好的"做成了"再盖一间厨房"。
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
