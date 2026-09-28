import { Suspense, lazy, useEffect, useState } from "react";

import { SubTabs } from "@/components/nav/SubTabs";
import { Placeholder } from "@/components/nav/Placeholder";
import { PageHeader } from "@/components/ui/page-header";
import { FridaPage } from "@/features/hook/frida/FridaPage";
import { useI18n } from "@/i18n";

/**
 * 真终端是**懒加载**的：xterm 家族（6 个 addon + 内核）min 之后约 823 kB，
 * 第七十五轮它被直接 import 进了主 JS 块（1,259 → 1,271 kB），启动关键路径上多了一整块
 * "只有点开才用得到的东西"。这里把它挪出主块：
 * - 默认（frida 子标签）不碰 `@/features/terminal/**`，Vite 因此把 xterm 切成独立 chunk；
 * - **一旦加载过就再也不卸载**——分页与会话保活靠的是"组件一直挂着"，
 *   卸载会把所有 PTY 会话一起收掉（那是 R-08 的反面）。
 */
const TerminalPage = lazy(() => import("@/features/terminal/TerminalPage").then((m) => ({ default: m.TerminalPage })));

/**
 * Hook 主标签（P10）：子标签化——frida 会话工作台 + 真终端 + 其余能力后续追加。
 */
export function HookPage() {
  const { t } = useI18n();
  const [sub, setSub] = useState("frida");
  // 只记"有没有加载过"，不记"现在是不是它"：切回 frida 时终端必须继续活着
  const [terminalLoaded, setTerminalLoaded] = useState(false);

  useEffect(() => {
    if (sub === "terminal") setTerminalLoaded(true);
  }, [sub]);

  return (
    <div className="flex h-full flex-col">
      <PageHeader title={t("nav.hook")} />
      <div className="min-h-0 flex-1">
        <SubTabs
          value={sub}
          onValueChange={setSub}
          tabs={[
            { id: "frida", label: t("hook.frida.tab"), content: <FridaPage /> },
            /* 真终端（FT0.3）：本机 PTY + xterm.js。它不是"另一个 frida 视图"，
               而是给路 B（官方 frida 命令行）准备的同一块地基。 */
            {
              id: "terminal",
              label: t("hook.terminal.tab"),
              content: terminalLoaded ? (
                <Suspense
                  fallback={
                    <div className="p-3 text-10px text-muted-foreground" data-testid="terminal-loading">
                      {t("hook.terminal.loading")}
                    </div>
                  }
                >
                  <TerminalPage />
                </Suspense>
              ) : (
                // 还没点开过：一块空占位（隐藏 tab，看不见也无所谓），只为让"没加载"可断言
                <div className="p-3 text-10px text-muted-foreground" data-testid="terminal-not-loaded" />
              ),
            },
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
