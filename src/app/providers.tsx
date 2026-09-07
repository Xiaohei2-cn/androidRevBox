import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

/** 主题偏好：浅色 / 深色 / 跟随系统 */
export type ThemePref = "light" | "dark" | "system";
export type EffectiveTheme = "light" | "dark";

interface SettingsContextValue {
  theme: ThemePref;
  setTheme: (theme: ThemePref) => void;
  effectiveTheme: EffectiveTheme;
  /** 窗口主体背景不透明度，20–100（%），UI 层钳制最小值防止全透 */
  opacity: number;
  setOpacity: (value: number) => void;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

const THEME_KEY = "app.settings.theme";
const OPACITY_KEY = "app.settings.opacity";
export const MIN_OPACITY = 20;
const DEFAULT_OPACITY = 100;

function readStored<T extends string>(
  key: string,
  allowed: readonly T[],
  fallback: T,
): T {
  try {
    const raw = localStorage.getItem(key);
    if (raw && (allowed as readonly string[]).includes(raw)) return raw as T;
  } catch {
    // localStorage 不可用时静默回退
  }
  return fallback;
}

function readStoredOpacity(): number {
  try {
    const raw = localStorage.getItem(OPACITY_KEY);
    if (raw === null) return DEFAULT_OPACITY;
    const parsed = Number(raw);
    if (Number.isFinite(parsed)) {
      return Math.min(100, Math.max(MIN_OPACITY, parsed));
    }
  } catch {
    // ignore
  }
  return DEFAULT_OPACITY;
}

function persist(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // P1 将迁移到 SQLite app_settings，此处失败可忽略
  }
}

function SettingsProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemePref>(() =>
    readStored(THEME_KEY, ["light", "dark", "system"] as const, "system"),
  );
  const [opacity, setOpacityState] = useState<number>(readStoredOpacity);
  const [systemDark, setSystemDark] = useState<boolean>(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);

  const effectiveTheme: EffectiveTheme =
    theme === "system" ? (systemDark ? "dark" : "light") : theme;

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", effectiveTheme === "dark");
    root.style.colorScheme = effectiveTheme;
  }, [effectiveTheme]);

  const setTheme = useCallback((next: ThemePref) => {
    setThemeState(next);
    persist(THEME_KEY, next);
  }, []);

  const setOpacity = useCallback((value: number) => {
    const clamped = Math.round(Math.min(100, Math.max(MIN_OPACITY, value)));
    setOpacityState(clamped);
    persist(OPACITY_KEY, String(clamped));
  }, []);

  const value = useMemo(
    () => ({ theme, setTheme, effectiveTheme, opacity, setOpacity }),
    [theme, setTheme, effectiveTheme, opacity, setOpacity],
  );

  return (
    <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>
  );
}

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings 必须在 AppProviders 内使用");
  return ctx;
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      staleTime: 30_000,
    },
  },
});

export function AppProviders({ children }: { children: ReactNode }) {
  return (
    <QueryClientProvider client={queryClient}>
      <SettingsProvider>
        <TooltipProvider delayDuration={200}>{children}</TooltipProvider>
      </SettingsProvider>
    </QueryClientProvider>
  );
}
