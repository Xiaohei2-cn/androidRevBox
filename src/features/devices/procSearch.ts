/**
 * `/proc/*` 详情里的搜索：命中区间计算 + 高亮切分。
 *
 * 纯函数、不碰 DOM，单测直接喂文本即可。两条规则值得写死在这里而不是散在组件里：
 * - 正则/普通文本两种模式共用同一条"空查询 = 不高亮"的短路，避免空串在正则下
 *   变成 `/g` 全量匹配、把每一行都切成一堆空片段；
 * - 0 宽匹配（`^`、`\b`、`x*`）不能渲染成高亮区间：它不覆盖任何字符，渲染出来是
 *   一根看不见的空 `<mark>`，只会让"命中 N 处"的数字骗人。
 */

export interface SearchOptions {
  /** 大小写敏感（正则模式下指给正则加不加 `i`） */
  caseSensitive: boolean;
  /** 把查询当正则 */
  regex: boolean;
}

export type Range = readonly [start: number, end: number];

export type Segment = { text: string; hit: boolean };

/** 编译查询串；空查询/非法正则返回 `null`（= 不搜索，不报错）。 */
export function compileSearch(
  query: string,
  options: SearchOptions,
): RegExp | null {
  if (!query) return null;
  const flags = `g${options.caseSensitive ? "" : "i"}`;
  try {
    const pattern = options.regex ? query : escapeRegExp(query);
    return new RegExp(pattern, flags);
  } catch {
    return null;
  }
}

/** 查询串本身是否非法（用于把错误提示摆到输入框下，而不是静默当没输入）。 */
export function isInvalidQuery(query: string, options: SearchOptions): boolean {
  return query.length > 0 && compileSearch(query, options) === null;
}

export function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** 一行里所有非空命中区间，按出现顺序。 */
export function findRanges(line: string, re: RegExp | null): Range[] {
  if (!re) return [];
  const ranges: Range[] = [];
  re.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(line)) !== null) {
    const start = match.index;
    const end = start + match[0].length;
    if (end > start) ranges.push([start, end]);
    // 0 宽匹配时 exec 不会自己前进，必须手动推进，否则死循环
    re.lastIndex = match[0].length === 0 ? start + 1 : end;
    if (re.lastIndex > line.length) break;
  }
  return ranges;
}

/** 把一行切成"普通 / 命中"片段，供渲染高亮。 */
export function splitLine(line: string, re: RegExp | null): Segment[] {
  const ranges = findRanges(line, re);
  if (ranges.length === 0) return [{ text: line, hit: false }];
  const segments: Segment[] = [];
  let cursor = 0;
  for (const [start, end] of ranges) {
    if (start > cursor) segments.push({ text: line.slice(cursor, start), hit: false });
    segments.push({ text: line.slice(start, end), hit: true });
    cursor = end;
  }
  if (cursor < line.length) segments.push({ text: line.slice(cursor), hit: false });
  return segments;
}

/** 整段文本的总命中数（0 宽匹配不计）。 */
export function countMatches(text: string, re: RegExp | null): number {
  if (!re) return 0;
  re.lastIndex = 0;
  let count = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    if (match[0].length > 0) count += 1;
    // 0 宽匹配：exec 在 lastIndex 上会原地踏步，必须手动 +1
    if (match[0].length === 0) re.lastIndex = match.index + 1;
    if (re.lastIndex > text.length) break;
  }
  return count;
}
