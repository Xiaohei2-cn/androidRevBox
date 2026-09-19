import { useMemo, type CSSProperties } from "react";
import { useSettings } from "@/app/providers";
import { AppNavProvider, useAppNav } from "@/app/nav";
import { MainTabRail } from "@/components/nav/MainTabRail";
import { TitleBar } from "@/components/window/TitleBar";
import { DashboardPage } from "@/features/dashboard/DashboardPage";
import { DevicesPage } from "@/features/devices/DevicesPage";
import { AdbPage } from "@/features/adb/AdbPage";
import { TrafficPage } from "@/features/traffic/TrafficPage";
import { HookPage } from "@/features/hook/HookPage";
import { BinaryPage } from "@/features/binary/BinaryPage";
import { UnpackPage } from "@/features/unpack/UnpackPage";
import { KernelPage } from "@/features/kernel/KernelPage";
import { MarketPage } from "@/features/market/MarketPage";
import { CryptoPage } from "@/features/crypto/CryptoPage";
import { PluginsPage } from "@/features/plugins/PluginsPage";
import { TasksPage } from "@/features/tasks/TasksPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { cn } from "@/lib/utils";

/** 窗口主体背景 RGB（与 globals.css 的色板保持一致） */
const BODY_BG_RGB: Record<"light" | "dark", string> = {
  light: "255 255 255",
  dark: "24 24 27",
};

export function AppShell() {
  return (
    <AppNavProvider>
      <ShellBody />
    </AppNavProvider>
  );
}

function ShellBody() {
  const { tab } = useAppNav();
  const { effectiveTheme, opacity } = useSettings();

  // CSS 变量供 .app-surface（主体 + 锯齿齿块）共用：同色、同透明度。
  // ⚠️ 不要给 .app-surface 加回 backdrop-filter/transform：WKWebView 合成层
  // 在重绘/悬停/前后台切换时销毁重建，透明窗口上表现为满 alpha 实色闪一帧
  // （P7 三轮闪烁反馈的根因，详见 PHASES §10.5.1）。

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
      <MainTabRail />
      <div
        data-testid="window-body"
        className="app-surface relative flex h-full min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border/60"
      >
        <TitleBar />
        {/* keep-mounted 切页（P7 闪烁二次修复）：
            - 七页常驻，非激活用 visibility:hidden 而非 display:none——后者会触发
              主体 .app-surface 合成层的失效重合成，透明窗口上表现为「闪成桌面」一帧；
              visibility 只改绘制，不失效祖先合成层。
            - 查询状态与任务会话跨 tab 保活，隐藏页轮询由 useActiveTab 暂停。 */}
        <main className="relative min-h-0 flex-1 overflow-hidden">
          <PageHost active={tab === "dashboard"}>
            <DashboardPage />
          </PageHost>
          <PageHost active={tab === "devices"}>
            <DevicesPage />
          </PageHost>
          <PageHost active={tab === "traffic"}>
            <TrafficPage />
          </PageHost>
          <PageHost active={tab === "terminal"}>
            <AdbPage />
          </PageHost>
          <PageHost active={tab === "hook"}>
            <HookPage />
          </PageHost>
          <PageHost active={tab === "binary"}>
            <BinaryPage />
          </PageHost>
          <PageHost active={tab === "unpack"}>
            <UnpackPage />
          </PageHost>
          <PageHost active={tab === "kernel"}>
            <KernelPage />
          </PageHost>
          <PageHost active={tab === "crypto"}>
            <CryptoPage />
          </PageHost>
          <PageHost active={tab === "plugins"}>
            <PluginsPage />
          </PageHost>
          <PageHost active={tab === "tasks"}>
            <TasksPage />
          </PageHost>
          <PageHost active={tab === "market"}>
            <MarketPage />
          </PageHost>
          <PageHost active={tab === "settings"}>
            <SettingsPage />
          </PageHost>
        </main>
      </div>
    </div>
  );
}

function PageHost({
  active,
  children,
}: {
  active: boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "absolute inset-0 overflow-hidden p-4",
        active ? "z-10" : "invisible",
      )}
      aria-hidden={!active}
    >
      {children}
    </div>
  );
}
