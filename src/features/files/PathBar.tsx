import { useEffect, useState, type KeyboardEvent } from "react";
import { ArrowLeft, ArrowRight, ArrowUp, CornerDownLeft, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import { normalizeRemotePath } from "./pathHistory";
import type { PathNav } from "./usePathHistory";

/**
 * 文件页地址栏：后退 / 前进 / 上一级 + 路径输入 + 刷新。
 *
 * 三个细节是有意的：
 * * 输入框里可以随便改，**回车或点"进入"才生效**——半途的路径不去请求设备，
 *   否则每敲一个字符都会打一次 adb；
 * * 后退/前进按浏览历史走（规则在 `pathHistory.ts`，被单测钉住），上一级按路径结构走；
 *   已经在根时上一级按钮禁用，而不是拼出一个 `/..` 之类的假路径；
 * * 键盘 Alt+←/→/↑ 与三个按钮同义（桌面应用里手比鼠标快）。
 */
export function PathBar({ nav, onRefresh }: { nav: PathNav; onRefresh: () => void }) {
  const { t } = useI18n();
  const [draft, setDraft] = useState(nav.path);
  // 用按钮或历史切换时输入框要跟上；用户正在手敲时不打断他
  useEffect(() => setDraft(nav.path), [nav.path]);

  const target = normalizeRemotePath(draft);
  const commit = () => {
    if (target) nav.go(target);
  };

  const onKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
      return;
    }
    if (!event.altKey) return;
    if (event.key === "ArrowLeft" && nav.canBack) {
      event.preventDefault();
      nav.back();
    } else if (event.key === "ArrowRight" && nav.canForward) {
      event.preventDefault();
      nav.forward();
    } else if (event.key === "ArrowUp" && nav.up) {
      event.preventDefault();
      nav.goUp();
    }
  };

  const navButton = {
    size: "sm",
    variant: "ghost",
    className: "h-8 w-8 shrink-0 px-0",
  } as const;

  return (
    <div className="flex shrink-0 items-center gap-1">
      <Button
        {...navButton}
        aria-label={t("devices.files.back")}
        title={t("devices.files.back")}
        disabled={!nav.canBack}
        onClick={nav.back}
      >
        <ArrowLeft className="h-3.5 w-3.5" />
      </Button>
      <Button
        {...navButton}
        aria-label={t("devices.files.forward")}
        title={t("devices.files.forward")}
        disabled={!nav.canForward}
        onClick={nav.forward}
      >
        <ArrowRight className="h-3.5 w-3.5" />
      </Button>
      <Button
        {...navButton}
        aria-label={t("devices.files.up")}
        title={t("devices.files.up")}
        disabled={!nav.up}
        onClick={nav.goUp}
      >
        <ArrowUp className="h-3.5 w-3.5" />
      </Button>
      <input
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={onKey}
        spellCheck={false}
        aria-label={t("devices.files.pathLabel")}
        placeholder={t("devices.files.pathLabel")}
        className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />
      <Button
        size="sm"
        variant="outline"
        className="h-8 shrink-0"
        disabled={!target || target === nav.path}
        onClick={commit}
      >
        <CornerDownLeft className="h-3.5 w-3.5" />
        {t("devices.files.enter")}
      </Button>
      <Button
        {...navButton}
        aria-label={t("devices.files.refresh")}
        title={t("devices.files.refresh")}
        onClick={onRefresh}
      >
        <RefreshCw className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}
