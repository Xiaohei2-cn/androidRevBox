import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { FridaPage } from "@/features/hook/frida/FridaPage";
import { TerminalPage } from "@/features/terminal/TerminalPage";
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
            /* 真终端（FT0.3）：本机 PTY + xterm.js。它不是"另一个 frida 视图"，
               而是给路 B（官方 frida 命令行）准备的同一块地基。 */
            { id: "terminal", label: t("hook.terminal.tab"), content: <TerminalPage /> },
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
