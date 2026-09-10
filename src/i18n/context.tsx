/**
 * 轻量 i18n（P8）——不引第三方库（§1.2 冻结基线未含 i18next）。
 *
 * 设计要点：
 * - 词典按**语言 + 域**分文件（locales/<lang>/<domain>.ts），新增功能在对应域追加键；
 * - `t("domain.key")` 扁平点号取键，缺失时回退：当前语言 → zh-CN → 原样返回键名
 *   （开发期一眼看出漏翻，生产期不会白屏）；
 * - 支持 `t("k", { n: 3 })` 的 `{n}` 插值；
 * - 语言集与后端 ConfigService::LOCALES 严格一致（新增语种需两处同时登记）。
 *
 * 本文件是核心机制（context + makeTranslate）；Provider/useI18n 见 I18nProvider.tsx。
 */

import { createContext, useContext } from "react";
import type { Locale } from "./dictionaries";

export type { Locale };

export interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: TranslateFn;
}

export type TranslateFn = (
  key: string,
  vars?: Record<string, string | number>,
) => string;

export const I18nContext = createContext<I18nContextValue | null>(null);

export function useI18nContext(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n 必须在 I18nProvider 内使用");
  return ctx;
}

/** 供 Provider 内部构造 t()（导出便于单测） */
export function makeTranslate(
  dict: Record<string, string>,
  fallback: Record<string, string>,
): TranslateFn {
  return (key, vars) => {
    const raw = dict[key] ?? fallback[key] ?? key;
    if (!vars) return raw;
    return raw.replace(/\{(\w+)\}/g, (m, name: string) =>
      vars[name] !== undefined ? String(vars[name]) : m,
    );
  };
}
