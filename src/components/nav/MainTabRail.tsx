import {
  Binary,
  LayoutDashboard,
  ListTodo,
  Puzzle,
  Settings,
  Smartphone,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAppNav } from "@/app/nav";
import { cn } from "@/lib/utils";

export type TabId =
  | "dashboard"
  | "devices"
  | "terminal"
  | "crypto"
  | "plugins"
  | "tasks"
  | "settings";

interface TabDef {
  id: TabId;
  label: string;
  icon: LucideIcon;
}

/** 左侧锯齿总 tab：纵向从上到下，凸出窗口主体左边缘 */
export const MAIN_TABS: readonly TabDef[] = [
  { id: "dashboard", label: "仪表盘", icon: LayoutDashboard },
  { id: "devices", label: "设备", icon: Smartphone },
  { id: "terminal", label: "终端", icon: SquareTerminal },
  { id: "crypto", label: "算法工具", icon: Binary },
  { id: "plugins", label: "插件中心", icon: Puzzle },
  { id: "tasks", label: "任务中心", icon: ListTodo },
  { id: "settings", label: "设置", icon: Settings },
] as const;

export function MainTabRail() {
  const { tab, setTab } = useAppNav();
  return (
    <nav
      aria-label="主导航"
      className="flex w-14 shrink-0 flex-col items-end gap-2 pt-14"
    >
      {MAIN_TABS.map((item) => (
        <MainTabButton
          key={item.id}
          tab={item}
          active={item.id === tab}
          onClick={() => setTab(item.id)}
        />
      ))}
    </nav>
  );
}

function MainTabButton({
  tab,
  active,
  onClick,
}: {
  tab: TabDef;
  active: boolean;
  onClick: () => void;
}) {
  const Icon = tab.icon;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={tab.label}
          aria-current={active ? "true" : undefined}
          onClick={onClick}
          className={cn(
            "group relative flex h-12 items-center justify-center rounded-l-xl border border-r-0 border-border/60 transition-all",
            active
              ? "w-14 bg-primary px-1 text-primary-foreground shadow-sm"
              : "app-surface w-6 text-muted-foreground hover:w-7 hover:text-foreground",
          )}
        >
          {active ? (
            <span className="whitespace-nowrap text-[11px] font-medium leading-none">
              {tab.label}
            </span>
          ) : (
            <Icon className="h-3.5 w-3.5" />
          )}
        </button>
      </TooltipTrigger>
      {!active && <TooltipContent side="right">{tab.label}</TooltipContent>}
    </Tooltip>
  );
}
