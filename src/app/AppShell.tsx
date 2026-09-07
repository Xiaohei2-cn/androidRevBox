import { useMemo, useState, type CSSProperties } from "react";
import { useSettings } from "@/app/providers";
import { MainTabRail, type TabId } from "@/components/nav/MainTabRail";
import { TitleBar } from "@/components/window/TitleBar";
import { DashboardPage } from "@/features/dashboard/DashboardPage";
import { DevicesPage } from "@/features/devices/DevicesPage";
import { TerminalPage } from "@/features/terminal/TerminalPage";
import { CryptoPage } from "@/features/crypto/CryptoPage";
import { PluginsPage } from "@/features/plugins/PluginsPage";
import { TasksPage } from "@/features/tasks/TasksPage";
import { SettingsPage } from "@/features/settings/SettingsPage";

/** 窗口主体背景 RGB（与 globals.css 的色板保持一致） */
const BODY_BG_RGB: Record<"light" | "dark", string> = {
  light: "255 255 255",
  dark: "24 24 27",
};

export function AppShell() {
  const [tab, setTab] = useState<TabId>("dashboard");
  const { effectiveTheme, opacity } = useSettings();

  // CSS 变量供 .app-surface（主体 + 锯齿齿块）共用：同色、同透明度、同毛玻璃
  const shellStyle = useMemo(
    () =>
      ({
        "--app-bg-rgb": BODY_BG_RGB[effectiveTheme],
        "--app-bg-alpha": String(opacity / 100),
      }) as CSSProperties,
    [effectiveTheme, opacity],
  );

  return (
    <div className="flex h-full w-full" style={shellStyle}>
      <MainTabRail active={tab} onChange={setTab} />
      <div
        data-testid="window-body"
        className="app-surface relative flex h-full min-w-0 flex-1 flex-col overflow-hidden rounded-r-xl border border-l-0 border-border/60 shadow-[0_8px_40px_rgba(0,0,0,0.35)]"
      >
        <TitleBar />
        <main className="min-h-0 flex-1 overflow-hidden p-4">{renderPage(tab)}</main>
      </div>
    </div>
  );
}

function renderPage(tab: TabId) {
  switch (tab) {
    case "dashboard":
      return <DashboardPage />;
    case "devices":
      return <DevicesPage />;
    case "terminal":
      return <TerminalPage />;
    case "crypto":
      return <CryptoPage />;
    case "plugins":
      return <PluginsPage />;
    case "tasks":
      return <TasksPage />;
    case "settings":
      return <SettingsPage />;
  }
}
