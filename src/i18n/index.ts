/**
 * 轻量 i18n（P8）公共出口。
 * - I18nProvider / useI18n：Provider 与组件 hook；
 * - 词典按语言+域分文件（locales/<lang>/<domain>.ts）；
 * - `t("domain.key", vars)`：缺失回退 当前语言 → zh-CN → 键名；支持 {var} 插值；
 * - 语言集与后端 ConfigService::LOCALES 严格一致（新增语种两处同时登记）。
 */

export { I18nProvider, useI18n } from "./I18nProvider";
export { makeTranslate, I18nContext, useI18nContext, type TranslateFn } from "./context";
export {
  LOCALES,
  DEFAULT_LOCALE,
  LOCALE_LABELS,
  detectLocale,
  isLocale,
  type Locale,
} from "./dictionaries";
