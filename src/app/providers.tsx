import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { configApi } from "@/api/config";
import { I18nProvider } from "@/i18n";

/** 主题偏好：浅色 / 深色 / 跟随系统 */
export type ThemePref = "light" | "dark" | "system";
export type EffectiveTheme = "light" | "dark";
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

/** 与 Rust ConfigService 白名单键一致 */
export const KEY_THEME = "app.settings.theme";
export const KEY_OPACITY = "app.settings.opacity";
export const KEY_LOG_LEVEL = "app.settings.log_level";

const THEME_VALUES: readonly string[] = ["light", "dark", "system"];
const LOG_LEVELS: readonly LogLevel[] = ["trace", "debug", "info", "warn", "error"];
export const MIN_OPACITY = 20;
const DEFAULT_OPACITY = 100;
const DEFAULT_LOG_LEVEL: LogLevel = "info";

interface SettingsContextValue {
  theme: ThemePref;
  setTheme: (theme: ThemePref) => void;
  effectiveTheme: EffectiveTheme;
  /** 窗口主体背景不透明度，20–100（%），UI 层钳制最小值防止全透 */
  opacity: number;
  setOpacity: (value: number) => void;
  logLevel: LogLevel;
  setLogLevel: (level: LogLevel) => void;
  /** 后端配置是否已从 SQLite 完成水合（P0 localStorage 用户在迁移完成前看到缓存值） */
  hydrated: boolean;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

/** 纯浏览器（无 Tauri IPC）时为 false：持久化退回 localStorage，保证测试与 vite 预览可用 */
function hasTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function readStoredTheme(): ThemePref {
  try {
    const raw = localStorage.getItem(KEY_THEME);
    if (raw && THEME_VALUES.includes(raw)) return raw as ThemePref;
  } catch {
    // localStorage 不可用时静默回退
  }
  return "system";
}

function readStoredOpacity(): number {
  try {
    const raw = localStorage.getItem(KEY_OPACITY);
    if (raw !== null) {
      const parsed = Number(raw);
      if (Number.isFinite(parsed)) {
        return Math.min(100, Math.max(MIN_OPACITY, parsed));
      }
    }
  } catch {
    // ignore
  }
  return DEFAULT_OPACITY;
}

function readStoredLogLevel(): LogLevel {
  try {
    const raw = localStorage.getItem(KEY_LOG_LEVEL);
    if (raw && LOG_LEVELS.includes(raw as LogLevel)) return raw as LogLevel;
  } catch {
    // ignore
  }
  return DEFAULT_LOG_LEVEL;
}

function persistCache(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 缓存失败不阻塞
  }
}

function isValidLogLevel(v: string): v is LogLevel {
  return LOG_LEVELS.includes(v as LogLevel);
}

function SettingsProvider({ children }: { children: ReactNode }) {
  // 初值取 localStorage 缓存：P1 前装机的用户立即可见自己的旧设置，之后异步被 SQLite 覆盖
  const [theme, setThemeState] = useState<ThemePref>(readStoredTheme);
  const [opacity, setOpacityState] = useState<number>(readStoredOpacity);
  const [logLevel, setLogLevelState] = useState<LogLevel>(readStoredLogLevel);
  const [hydrated, setHydrated] = useState(false);
  const [systemDark, setSystemDark] = useState<boolean>(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );

  // 水合期间的 set 走“跳过写回”，避免把缓存值误写回 SQLite
  const hydrating = useRef(!hasTauri() ? false : true);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);

  // 启动水合：SQLite 是事实源；localStorage 里的旧值若 DB 缺失则一次性迁移上去
  useEffect(() => {
    if (!hasTauri()) {
      setHydrated(true);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const rows = await configApi.snapshot();
        if (cancelled) return;
        const db = new Map(rows.map((r) => [r.key, r.value]));

        const migrateUp = async (key: string, cacheValue: string) => {
          if (!db.has(key)) {
            try {
              if (key === KEY_LOG_LEVEL) {
                await configApi.setLogLevel(cacheValue);
              } else {
                await configApi.set(key, cacheValue);
              }
              db.set(key, cacheValue);
            } catch {
              // 非法缓存值写不进去就跳过，落回默认
            }
          }
        };
        await migrateUp(KEY_THEME, theme);
        await migrateUp(KEY_OPACITY, String(opacity));
        await migrateUp(KEY_LOG_LEVEL, logLevel);
        if (cancelled) return;

        const dbTheme = db.get(KEY_THEME);
        if (dbTheme && THEME_VALUES.includes(dbTheme)) {
          setThemeState(dbTheme as ThemePref);
          persistCache(KEY_THEME, dbTheme);
        }
        const dbOpacity = Number(db.get(KEY_OPACITY));
        if (Number.isFinite(dbOpacity)) {
          const clamped = Math.min(100, Math.max(MIN_OPACITY, dbOpacity));
          setOpacityState(clamped);
          persistCache(KEY_OPACITY, String(clamped));
        }
        const dbLevel = db.get(KEY_LOG_LEVEL);
        if (dbLevel && isValidLogLevel(dbLevel)) {
          setLogLevelState(dbLevel);
          persistCache(KEY_LOG_LEVEL, dbLevel);
        }
      } catch (e) {
        // 后端不可用时保留 localStorage 缓存值，不阻塞 UI
        console.error("配置水合失败，使用本地缓存", e);
      } finally {
        if (!cancelled) {
          hydrating.current = false;
          setHydrated(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // 只在挂载时执行一次；闭包内用的是初值
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const effectiveTheme: EffectiveTheme =
    theme === "system" ? (systemDark ? "dark" : "light") : theme;

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", effectiveTheme === "dark");
    root.style.colorScheme = effectiveTheme;
  }, [effectiveTheme]);

  /** 写回：缓存同步、SQLite/级别联动异步 fire-and-forget；水合期间不回写避免覆盖 */
  const writeThrough = useCallback((key: string, value: string) => {
    persistCache(key, value);
    if (!hasTauri() || hydrating.current) return;
    const task =
      key === KEY_LOG_LEVEL
        ? configApi.setLogLevel(value).then(() => null)
        : configApi.set(key, value);
    task.catch((e) => console.error(`设置持久化失败: ${key}`, e));
  }, []);

  const setTheme = useCallback(
    (next: ThemePref) => {
      setThemeState(next);
      writeThrough(KEY_THEME, next);
    },
    [writeThrough],
  );

  const setOpacity = useCallback(
    (value: number) => {
      const clamped = Math.round(Math.min(100, Math.max(MIN_OPACITY, value)));
      setOpacityState(clamped);
      writeThrough(KEY_OPACITY, String(clamped));
    },
    [writeThrough],
  );

  const setLogLevel = useCallback(
    (next: LogLevel) => {
      setLogLevelState(next);
      writeThrough(KEY_LOG_LEVEL, next);
    },
    [writeThrough],
  );

  const value = useMemo(
    () => ({
      theme,
      setTheme,
      effectiveTheme,
      opacity,
      setOpacity,
      logLevel,
      setLogLevel,
      hydrated,
    }),
    [theme, setTheme, effectiveTheme, opacity, setOpacity, logLevel, setLogLevel, hydrated],
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
        <I18nProvider>
          <TooltipProvider delayDuration={200}>{children}</TooltipProvider>
        </I18nProvider>
      </SettingsProvider>
    </QueryClientProvider>
  );
}
