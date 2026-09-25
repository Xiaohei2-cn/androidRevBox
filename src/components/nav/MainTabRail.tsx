import {
  Activity,
  Binary,
  Bug,
  FileArchive,
  LayoutDashboard,
  ListTodo,
  Package,
  Puzzle,
  Settings,
  Smartphone,
  Store,
  Usb,
  Waypoints,
  type LucideIcon,
} from "lucide-react";
import { useAppNav } from "@/app/nav";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export type TabId =
  | "dashboard"
  | "devices"
  | "traffic"
  | "terminal"
  | "hook"
  | "binary"
  | "unpack"
  | "kernel"
  | "crypto"
  | "plugins"
  | "tasks"
  | "market"
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
  { id: "traffic", labelKey: "nav.traffic", icon: Activity },
  { id: "terminal", labelKey: "nav.terminal", icon: Usb },
  { id: "hook", labelKey: "nav.hook", icon: Bug },
  { id: "binary", labelKey: "nav.binary", icon: Binary },
  { id: "unpack", labelKey: "nav.unpack", icon: Package },
  { id: "kernel", labelKey: "nav.kernel", icon: FileArchive },
  { id: "crypto", labelKey: "nav.crypto", icon: Waypoints },
  { id: "plugins", labelKey: "nav.plugins", icon: Puzzle },
  { id: "tasks", labelKey: "nav.tasks", icon: ListTodo },
  { id: "market", labelKey: "nav.market", icon: Store },
  { id: "settings", labelKey: "nav.settings", icon: Settings },
] as const;

export function MainTabRail() {
  const { tab, setTab } = useAppNav();
  const { t } = useI18n();
  return (
    <nav
      aria-label={t("nav.aria")}
      // 全项目**唯一**声明"可以让出点击"的区域：rail 宽 128px 而齿块只占右边 36px，
      // 左边那一条透明留白与齿块之间的缝隙就是用户要点穿到后面 App 的地方。
      // 判定默认是"接住"，所以别的界面不标也不会被误穿；这里显式标 pass 才让出去。
      data-click-through="pass"
      className="flex w-32 shrink-0 flex-col items-end gap-1.5 overflow-y-auto pt-14"
    >
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
 * UI 统一改版：未选中齿块加宽到 36px、图标 16px（旧版 28px/14px 视觉权重过低，
 * 像缩在角落）；悬停以浅底反馈而非拉宽（拉宽会重采样合成层，保持即时）；
 * 选中标签升回正文字号，与内容区标题体系对齐。
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
      // 未选中齿块本体只有那层半透明着色 → 让出去；图标热区标回 solid（否则切不了页）。
      // 选中态是实心 bg-primary + 中文标签，本来就是实体：**必须显式标 solid**，
      // 不然会被 nav 的 pass 一路穿透掉，当前页的 tab 就成了点不动的空壳。
      data-click-through={active ? "solid" : "pass"}
      onClick={onClick}
      className={cn(
        "app-surface group relative flex h-10 shrink-0 select-none items-center justify-center rounded-l-xl border border-r-0 border-border/60",
        active
          ? "max-w-28 bg-primary px-3 text-primary-foreground"
          : "w-9 text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {active ? (
        <span className="max-w-full truncate text-sm font-medium leading-none">
          {label}
        </span>
      ) : (
        /*
         * 图标外面这块 pad 是齿块上唯一的实体热区：24×24 居中，占 36×40 齿块的大部分，
         * 剩下那一圈透明边才让给后面的 App。热区大小想调就改这里（h-6/w-6）。
         */
        <span data-click-through="solid" className="grid h-6 w-6 shrink-0 place-items-center">
          <Icon className="h-4 w-4" />
        </span>
      )}
    </button>
  );
}
