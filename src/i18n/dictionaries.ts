/**
 * 词典加载与合并（P8）。
 *
 * 目录约定：`locales/<locale>/<domain>.ts`，每个文件 `export default { "domain.key": "…" }`。
 * 新增功能时：在**所有**语言目录的同名域文件里补齐键（CI 用 `pnpm i18n:check` 校验完整性）。
 *
 * 回退链：当前语言 → zh-CN → 键名本身。zh-CN 是源语言，必须最全。
 */

import en from "./locales/en";
import ja from "./locales/ja";
import ptBR from "./locales/pt-BR";
import ru from "./locales/ru";
import zhCN from "./locales/zh-CN";

/** 与 Rust `ConfigService::LOCALES` 一一对应（新增语种两处都要登记） */
export const LOCALES = ["zh-CN", "en", "ru", "pt-BR", "ja"] as const;
export type Locale = (typeof LOCALES)[number];

export const DEFAULT_LOCALE: Locale = "zh-CN";

/** 语言标签用各语言自身书写（language endonym），避免「看不懂的选项」 */
export const LOCALE_LABELS: Record<Locale, string> = {
  "zh-CN": "简体中文",
  en: "English",
  ru: "Русский",
  "pt-BR": "Português (Brasil)",
  ja: "日本語",
};

export const DICTIONARIES: Record<Locale, Record<string, string>> = {
  "zh-CN": zhCN,
  en,
  ru,
  "pt-BR": ptBR,
  ja,
};

export function isLocale(v: string): v is Locale {
  return (LOCALES as readonly string[]).includes(v);
}

/** 浏览器/系统语言的近似匹配（无匹配则默认 zh-CN） */
export function detectLocale(): Locale {
  const nav = typeof navigator !== "undefined" ? navigator.language : "";
  if (!nav) return DEFAULT_LOCALE;
  if (isLocale(nav)) return nav;
  const lower = nav.toLowerCase();
  // 语言前缀兜底：zh-TW→zh-CN、pt-PT→pt-BR、en-GB→en…
  const byPrefix = LOCALES.find((l) => l.toLowerCase().startsWith(lower.split("-")[0]));
  return byPrefix ?? DEFAULT_LOCALE;
}
