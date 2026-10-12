/**
 * 算法清单的多语言标签（CyberChef 移植版的关键约束：清单必须跟着设置里的语言走）。
 *
 * 结构分工：
 * - Rust 注册表给**结构与默认值**（id / 分类 / 参数名 / 候选值）；
 * - 前端词典给**文案**（`chef.op.<id>.name`、`chef.category.<id>`、`chef.arg.<name>`、`chef.opt.<token>`）。
 *
 * 为什么文案不放 Rust：`pnpm i18n:check` 只能校验前端词典的五语一致性，
 * 放后端就等于漏翻没人拦得住。
 *
 * 键是动态拼的（词典检查脚本看不见），所以这里两件事一起做：
 * 1) 取不到译文时**回退**成 id / 原始 token，界面上绝不会显示 `chef.op.xxx.name` 这种键名；
 * 2) `chefCatalogue.test.ts` 从 Rust 源码里抽出全部 id/分类/参数，逐条断言五种语言都有词。
 */

import { useMemo } from "react";

import { DICTIONARIES, type Locale, type TranslateFn, useI18n } from "@/i18n";

const LOCALES = Object.keys(DICTIONARIES) as Locale[];

/** 词典里是否真的有这个键（回退到 zh-CN 也算有：宁可给中文也不给键名） */
function has(dict: Record<string, string>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(dict, key);
}

export type ChefLabels = {
  locale: Locale;
  operation: (id: string) => string;
  description: (id: string) => string;
  category: (id: string) => string;
  argument: (name: string) => string;
  option: (token: string) => string;
  /**
   * 输入解释方式 / 输出口径的显示名。
   * 为什么不复用 option()：同一个 token 在不同上下文里意思不同——`text` 在排序里是
   * "按文本"，在输出面板里是"文本"；`hex` 在参数里是候选值，在面板里是显示口径。
   * 共用 `chef.opt.*` 必然有一处译错，所以给面板单独一层键（`chef.view.*`），
   * 取不到再退回 `chef.opt.*`。
   */
  view: (token: string) => string;
  /** 界面外壳文案（键在 chef.ui.* 下）；取不到就显示键名本身，不显示完整键 */
  ui: (key: string, vars?: Record<string, string | number>) => string;
};

/**
 * 返回值的**对象引用按 locale 记忆化**。
 * 这不是洁癖：清单有 120 条目，每次执行结果回来都会重渲染工作台，
 * 如果 labels 每次都是新对象，Catalogue 的 useMemo 依赖就永远在变，
 * 于是"跑一次算法"会把 120 条目的搜索索引全部重算并重绘一遍（透明窗口下
 * 这就是用户看到的闪）。见 docs/algorithm_center.md 的闪烁一节。
 */
export function useChefLabels(): ChefLabels {
  const { t, locale } = useI18n();
  return useMemo(() => buildLabels(locale, t), [locale, t]);
}

function buildLabels(locale: Locale, t: TranslateFn): ChefLabels {
  const dict = DICTIONARIES[locale];
  const fallbackDict = DICTIONARIES["zh-CN"];

  const lookup = (key: string, fallback: string) =>
    has(dict, key) || has(fallbackDict, key) ? t(key) : fallback;

  return {
    locale,
    operation: (id) => lookup(`chef.op.${id}.name`, id),
    description: (id) => lookup(`chef.op.${id}.desc`, ""),
    category: (id) => lookup(`chef.category.${id}`, id),
    argument: (name) => lookup(`chef.arg.${name}`, name),
    option: (token) => lookup(`chef.opt.${token}`, token),
    view: (token) =>
      has(dict, `chef.view.${token}`) || has(fallbackDict, `chef.view.${token}`)
        ? t(`chef.view.${token}`)
        : lookup(`chef.opt.${token}`, token),
    ui: (key, vars) => {
      const full = `chef.ui.${key}`;
      return has(dict, full) || has(fallbackDict, full) ? t(full, vars) : key;
    },
  };
}

/**
 * 清单里出现的**全部**文本（名称 + 说明 + 分类 + id）拼成一条小写串给搜索框用。
 * 搜「md5」要能命中「MD5 摘要」，搜「摘要」也要能命中——语言切换后同样成立。
 */
export function searchHaystack(parts: (string | undefined)[]): string {
  return parts
    .filter((part): part is string => Boolean(part))
    .join(" ")
    .toLocaleLowerCase("en-US");
}

/**
 * 清单项的悬停提示。
 * CyberChef 的清单只有一行名字，说明放在右侧详情里；我们不做右侧详情，
 * 于是：有 `chef.op.<id>.desc` 就用它，否则把**本地化后的参数名**当提示
 * （参数名本来就在词典里，等于免费拿到一句"这个算法要填什么"）。
 */
export function operationHint(
  labels: ChefLabels,
  operation: { id: string; args: { name: string }[] },
): string {
  const description = labels.description(operation.id);
  if (description) return description;
  const name = labels.operation(operation.id);
  const shown = operation.args.slice(0, 4).map((arg) => labels.argument(arg.name));
  if (shown.length === 0) return name;
  const rest = operation.args.length - shown.length;
  return `${name} - ${shown.join(", ")}${rest > 0 ? ` +${rest}` : ""}`;
}

/** 词典覆盖度自检用：某个键在五份词典里是不是都有 */
export function keyCoverageAcrossLocales(key: string): { locale: Locale; present: boolean }[] {
  return LOCALES.map((locale) => ({
    locale,
    present: has(DICTIONARIES[locale], key),
  }));
}
