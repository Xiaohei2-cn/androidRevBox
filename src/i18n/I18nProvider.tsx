/**
 * I18nProvider（P8）：locale 状态 + 持久化。
 * 策略与主题一致：SQLite（app.settings.locale）为事实源，localStorage 作启动
 * 缓存消除首帧闪烁；DB 缺键时一次性迁移回写。默认按系统语言探测。
 */

import { useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  DICTIONARIES,
  DEFAULT_LOCALE,
  detectLocale,
  isLocale,
  type Locale,
} from "./dictionaries";
import { I18nContext, makeTranslate } from "./context";

const LOCAL_CACHE_KEY = "app.settings.locale";

function readCachedLocale(): Locale {
  try {
    const raw = localStorage.getItem(LOCAL_CACHE_KEY);
    if (raw && isLocale(raw)) return raw;
  } catch {
    // localStorage 不可用时静默回退
  }
  return detectLocale();
}

function persistCache(locale: Locale) {
  try {
    localStorage.setItem(LOCAL_CACHE_KEY, locale);
  } catch {
    // 缓存失败不阻塞
  }
}

function hasTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(readCachedLocale);

  // 启动水合：SQLite 里的显式选择覆盖系统探测；DB 无键时把探测结果回写
  useEffect(() => {
    if (!hasTauri()) return;
    let cancelled = false;
    (async () => {
      try {
        const { configApi } = await import("@/api/config");
        const rows = await configApi.snapshot();
        if (cancelled) return;
        const row = rows.find((r) => r.key === "app.settings.locale");
        if (row && isLocale(row.value) && row.value !== locale) {
          setLocaleState(row.value);
          persistCache(row.value);
        } else if (!row) {
          configApi.set("app.settings.locale", locale).catch(() => undefined);
        }
      } catch {
        // 后端不可用：保留本地缓存值
      }
    })();
    return () => {
      cancelled = true;
    };
    // 只在挂载时执行一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    persistCache(next);
    if (hasTauri()) {
      import("@/api/config")
        .then(({ configApi }) =>
          configApi.set("app.settings.locale", next).catch(() => undefined),
        )
        .catch(() => undefined);
    }
  }, []);

  const t = useMemo(
    () => makeTranslate(DICTIONARIES[locale], DICTIONARIES[DEFAULT_LOCALE]),
    [locale],
  );

  const value = useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** 便捷 hook：组件内取 t() 与 locale */
export function useI18n() {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n 必须在 I18nProvider 内使用");
  return ctx;
}
