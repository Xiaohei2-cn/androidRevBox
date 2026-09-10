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
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
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
  /** i18n 键（nav.*） */
  labelKey: string;
  icon: LucideIcon;
}

/** 左侧锯齿总 tab：纵向从上到下，凸出窗口主体左边缘 */
export const MAIN_TABS: readonly TabDef[] = [
  { id: "dashboard", labelKey: "nav.dashboard", icon: LayoutDashboard },
  { id: "devices", labelKey: "nav.devices", icon: Smartphone },
  { id: "terminal", labelKey: "nav.terminal", icon: SquareTerminal },
  { id: "crypto", labelKey: "nav.crypto", icon: Binary },
  { id: "plugins", labelKey: "nav.plugins", icon: Puzzle },
  { id: "tasks", labelKey: "nav.tasks", icon: ListTodo },
  { id: "settings", labelKey: "nav.settings", icon: Settings },
] as const;

export function MainTabRail() {
  const { tab, setTab } = useAppNav();
  const { t } = useI18n();
  return (
    <nav aria-label={t("nav.aria")} className="flex w-14 shrink-0 flex-col items-end gap-2 pt-14">
      {MAIN_TABS.map((item) => (
        <MainTabButton
          key={item.id}
          tab={item}
          label={t(item.labelKey)}
          active={item.id === tab}
          onClick={() => setTab(item.id)}
        />
      ))}
    </nav>
  );
}

/**
 * 齿块（P7 闪烁二次修复）：
 * - `.app-surface`（毛玻璃层）恒挂在所有齿块上，选中态只叠加不透明 bg-primary 盖住它。
 *   此前选中/未选中靠增删 app-surface 类切换——在 macOS 透明窗口上意味着
 *   backdrop-filter 合成层的销毁/重建，每次切 tab 整窗闪一下（快照空帧）。
 * - 不做宽度 transition：backdrop-filter 区域逐帧变化同样触发重采样；
 *   悬停/选中的增宽即时生效。
 * - 不使用气泡提示（用户反馈：选中态本身就是中文标签）。
 */
function MainTabButton({
  tab,
  label,
  active,
  onClick,
}: {
  tab: TabDef;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  const Icon = tab.icon;
  return (
    <button
      type="button"
      aria-label={label}
      aria-current={active ? "true" : undefined}
      onClick={onClick}
      className={cn(
        "app-surface group relative flex h-12 items-center justify-center rounded-l-xl border border-r-0 border-border/60",
        active
          ? "w-14 bg-primary px-1 text-primary-foreground shadow-sm"
          : "w-6 text-muted-foreground hover:w-7 hover:text-foreground",
      )}
    >
      {active ? (
        <span className="whitespace-nowrap text-[11px] font-medium leading-none">
          {label}
        </span>
      ) : (
        <Icon className="h-3.5 w-3.5" />
      )}
    </button>
  );
}
