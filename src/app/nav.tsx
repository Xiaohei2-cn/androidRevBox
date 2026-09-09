import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { TabId } from "@/components/nav/MainTabRail";

/**
 * 全局导航上下文（P7）：
 * - gotoConfig(key)：任意卡片跳到 设置 页并让对应配置输入框聚焦 + 蓝框闪烁两下；
 * - useActiveTab：keep-mounted 的页面用它暂停后台轮询（§10 减少开销）。
 * 默认值为「仪表盘激活 + 空操作」，组件可脱离 Provider 单独渲染测试。
 */
interface AppNav {
  tab: TabId;
  setTab: (t: TabId) => void;
  /** 待高亮的设置键（null = 无） */
  pendingConfigKey: string | null;
  gotoConfig: (key: string) => void;
  clearPendingConfig: () => void;
}

const DEFAULT_NAV: AppNav = {
  tab: "dashboard",
  setTab: () => undefined,
  pendingConfigKey: null,
  gotoConfig: () => undefined,
  clearPendingConfig: () => undefined,
};

const NavContext = createContext<AppNav>(DEFAULT_NAV);

export function AppNavProvider({ children }: { children: ReactNode }) {
  const [tab, setTab] = useState<TabId>("dashboard");
  const [pendingConfigKey, setPendingConfigKey] = useState<string | null>(null);

  const gotoConfig = useCallback((key: string) => {
    setTab("settings");
    setPendingConfigKey(key);
  }, []);
  const clearPendingConfig = useCallback(() => setPendingConfigKey(null), []);

  const value = useMemo<AppNav>(
    () => ({ tab, setTab, pendingConfigKey, gotoConfig, clearPendingConfig }),
    [tab, pendingConfigKey, gotoConfig, clearPendingConfig],
  );
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useAppNav(): AppNav {
  return useContext(NavContext);
}

/** 当前主 tab 是否为 target（用于暂停隐藏页的轮询/探测） */
export function useActiveTab(target: TabId): boolean {
  return useAppNav().tab === target;
}
