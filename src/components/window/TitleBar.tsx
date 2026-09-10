import { getCurrentWindow } from "@tauri-apps/api/window";
import { useI18n } from "@/i18n";
import { Minus, Square, X } from "lucide-react";

/** 浏览器直开 dev server 时无 Tauri internals，惰性获取避免整页崩溃 */
function getWindow(): ReturnType<typeof getCurrentWindow> | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

/** macOS 用红黄绿圆点（左侧），Windows/Linux 用最小化/最大化/关闭（右侧） */
const isMacLike = navigator.userAgent.includes("Macintosh");

export function TitleBar() {
  const { t } = useI18n();
  return (
    <header
      data-tauri-drag-region
      className="flex h-10 shrink-0 items-center gap-3 border-b border-border/60"
    >
      {/* macOS：控制按钮在左（对齐系统习惯，红黄绿顺序）；拖拽区在其右 */}
      {isMacLike && <WindowControls />}
      <div
        data-tauri-drag-region
        className="pointer-events-none flex flex-1 items-baseline gap-2 pl-4"
      >
        <span className="text-sm font-semibold tracking-tight">
          {t("app.name")}
        </span>
        <span className="text-xs text-muted-foreground">{t("app.tagline")}</span>
      </div>
      {/* Windows/Linux：控制按钮在右（关闭最右，符合平台习惯） */}
      {!isMacLike && <WindowControls />}
    </header>
  );
}

function WindowControls() {
  if (isMacLike) {
    return (
      <div className="flex shrink-0 items-center gap-2 pl-4 pr-1">
        {/* 红黄绿顺序与系统一致；关闭在最左 */}
        <ControlDot
          label="关闭窗口"
          color="bg-[#ff5f57]"
          onClick={() => void getWindow()?.close()}
        />
        <ControlDot
          label="最小化窗口"
          color="bg-[#febc2e]"
          onClick={() => void getWindow()?.minimize()}
        />
        <ControlDot
          label="切换最大化"
          color="bg-[#28c840]"
          onClick={() => void getWindow()?.toggleMaximize()}
        />
      </div>
    );
  }
  return (
    <div className="flex h-full shrink-0 items-stretch">
      <ControlButton label="最小化窗口" onClick={() => void getWindow()?.minimize()}>
        <Minus className="h-3.5 w-3.5" />
      </ControlButton>
      <ControlButton
        label="切换最大化"
        onClick={() => void getWindow()?.toggleMaximize()}
      >
        <Square className="h-3 w-3" />
      </ControlButton>
      <ControlButton
        label="关闭窗口"
        danger
        onClick={() => void getWindow()?.close()}
      >
        <X className="h-3.5 w-3.5" />
      </ControlButton>
    </div>
  );
}

function ControlDot({
  label,
  color,
  onClick,
}: {
  label: string;
  color: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`h-3 w-3 rounded-full ring-1 ring-black/10 transition-[filter] hover:brightness-90 ${color}`}
    />
  );
}

function ControlButton({
  label,
  danger,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`flex w-12 items-center justify-center text-muted-foreground transition-colors hover:bg-black/10 hover:text-foreground dark:hover:bg-white/10 ${
        danger ? "hover:bg-red-600 hover:text-white dark:hover:bg-red-600" : ""
      }`}
    >
      {children}
    </button>
  );
}
