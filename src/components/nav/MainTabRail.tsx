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
      // nav 自己**不带**任何穿透标记：默认就是"接住点击"。
      // 之前整条栏标 pass，把齿块之间那 6px 缝（gap-1.5）也一起送了出去 —— 用户要的
      // 只是"透传 tab 底图"，缝隙属于栏的 chrome，该接住。
      // 顶部那 56px 仍然用 nav 自己的 padding 撑：它属于 nav、没有声明 → 接住点击。
      // （不写成一条 flex 子元素是有原因的：nav 的 gap 会在它与第一个 tab 之间多塞 6px，
      //  整条栏被往下推，与内容区顶边错位。）
      className="flex w-32 shrink-0 flex-col gap-1.5 overflow-y-auto pt-14"
    >
      {MAIN_TABS.map((item) => (
        <div key={item.id} className="flex w-full items-stretch justify-end">
          {/*
            本行齿块左边的透明留白：全项目**唯一**让出点击的地方。按行声明，所以滚动、
            选中态齿块变宽都不会错位。齿块本身（含图标和它下面那一圈）、齿块之间那 6px 缝、
            行容器与 nav 都不标 → 一律接住：点 tab 绝不能穿到后面的 App 去。
          */}
          <span aria-hidden data-click-through="pass" className="min-w-0 flex-1" />
          <MainTabButton
            tab={item}
            label={t(item.labelKey)}
            active={item.id === tab}
            onClick={() => setTab(item.id)}
          />
        </div>
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
      // 整个齿块一律接住点击（**不标 pass，也不留小热区**）：它就是"点哪个 tab"的目标。
      // 上一版把未选中齿块本体标成穿透、只在图标外留 24×24 热区，结果齿块 36×40 居中
      // 一个 24×24 → 图标下面正好空着 8px：想点 tab 却穿到后面的 App，直接影响操作。
      // 穿透只保留在齿块**左边**那条留白上（见下面的行内 span）。
      onClick={onClick}
      className={cn(
        "app-surface group relative flex h-10 shrink-0 select-none items-center justify-center self-end rounded-l-xl border border-r-0 border-border/60",
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
        <Icon className="h-4 w-4" />
      )}
    </button>
  );
}
