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
 * 全局导航上下文（P7/P8）：
 * - gotoConfig(key)：任意卡片跳到 设置 页并让对应配置输入框聚焦 + 蓝框闪烁两下；
 * - gotoDeviceList()：跳到 设备 页的设备列表 tab（仪表盘 ADB 卡导航）；
 * - gotoDeviceInfo(serial)：跳到 设备 页的设备信息 tab 并定位该设备卡片；
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
  /** 待定位的设备 serial（设备信息 tab；null = 无） */
  pendingDeviceSerial: string | null;
  gotoDeviceInfo: (serial: string) => void;
  clearPendingDevice: () => void;
  /** 设备页子 tab 请求（列表/信息；null = 用默认） */
  pendingDevicesTab: "list" | "info" | null;
  gotoDeviceList: () => void;
  /** ADB 页子 tab 请求（P10 frida 前置检查深链：端口转发） */
  pendingAdbTab: "forward" | null;
  gotoAdbSubTab: (tab: "forward") => void;
  /**
   * 「二进制」页子 tab 请求（so 替换 / 二进制托管）。
   * 托管原来挂在 ADB 页下，frida 的修复深链也跟着指那边；主 tab 归位后深链必须一起
   * 改指，否则点「去托管 frida-server」会跳到一个已经没有托管入口的页面。
   */
  pendingBinaryTab: "so-replace" | "hosting" | null;
  gotoBinarySubTab: (tab: "so-replace" | "hosting") => void;
  /** 任务中心（P10 会话头「跳任务中心」） */
  gotoTasks: () => void;
}

const DEFAULT_NAV: AppNav = {
  tab: "dashboard",
  setTab: () => undefined,
  pendingConfigKey: null,
  gotoConfig: () => undefined,
  clearPendingConfig: () => undefined,
  pendingDeviceSerial: null,
  gotoDeviceInfo: () => undefined,
  clearPendingDevice: () => undefined,
  pendingDevicesTab: null,
  gotoDeviceList: () => undefined,
  pendingAdbTab: null,
  gotoAdbSubTab: () => undefined,
  pendingBinaryTab: null,
  gotoBinarySubTab: () => undefined,
  gotoTasks: () => undefined,
};

const NavContext = createContext<AppNav>(DEFAULT_NAV);

export function AppNavProvider({ children }: { children: ReactNode }) {
  const [tab, setTab] = useState<TabId>("dashboard");
  const [pendingConfigKey, setPendingConfigKey] = useState<string | null>(null);
  const [pendingDeviceSerial, setPendingDeviceSerial] = useState<string | null>(null);
  const [pendingDevicesTab, setPendingDevicesTab] = useState<"list" | "info" | null>(null);
  const [pendingAdbTab, setPendingAdbTab] = useState<"forward" | null>(null);
  const [pendingBinaryTab, setPendingBinaryTab] = useState<"so-replace" | "hosting" | null>(null);

  const gotoConfig = useCallback((key: string) => {
    setTab("settings");
    setPendingConfigKey(key);
  }, []);
  const clearPendingConfig = useCallback(() => setPendingConfigKey(null), []);

  const gotoDeviceInfo = useCallback((serial: string) => {
    setTab("devices");
    setPendingDeviceSerial(serial);
    setPendingDevicesTab("info");
  }, []);
  const clearPendingDevice = useCallback(() => setPendingDeviceSerial(null), []);

  const gotoDeviceList = useCallback(() => {
    setTab("devices");
    setPendingDevicesTab("list");
  }, []);

  const gotoAdbSubTab = useCallback((next: "forward") => {
    setTab("terminal");
    setPendingAdbTab(next);
  }, []);

  const gotoBinarySubTab = useCallback((next: "so-replace" | "hosting") => {
    setTab("binary");
    setPendingBinaryTab(next);
  }, []);

  const gotoTasks = useCallback(() => setTab("tasks"), []);

  const value = useMemo<AppNav>(
    () => ({
      tab,
      setTab,
      pendingConfigKey,
      gotoConfig,
      clearPendingConfig,
      pendingDeviceSerial,
      gotoDeviceInfo,
      clearPendingDevice,
      pendingDevicesTab,
      gotoDeviceList,
      pendingAdbTab,
      gotoAdbSubTab,
      pendingBinaryTab,
      gotoBinarySubTab,
      gotoTasks,
    }),
    [
      tab,
      pendingConfigKey,
      gotoConfig,
      clearPendingConfig,
      pendingDeviceSerial,
      gotoDeviceInfo,
      clearPendingDevice,
      pendingDevicesTab,
      gotoDeviceList,
      pendingAdbTab,
      gotoAdbSubTab,
      pendingBinaryTab,
      gotoBinarySubTab,
      gotoTasks,
    ],
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
