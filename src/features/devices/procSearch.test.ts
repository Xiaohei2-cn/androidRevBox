import { describe, expect, it } from "vitest";
import {
  compileSearch,
  countMatches,
  findRanges,
  isInvalidQuery,
  splitLine,
} from "./procSearch";

/**
 * `/proc` 详情搜索的纯逻辑契约：
 * ① 默认忽略大小写，开了区分大小写就严格匹配；
 * ② 普通模式下 `.` `(` 这类字符按字面量算，不能被当成正则；
 * ③ 非法正则不抛错，返回 null 并由 isInvalidQuery 报给界面；
 * ④ 0 宽匹配（`^` `\b` `x*`）不产生高亮，也不能让循环卡死。
 */

const CI = { caseSensitive: false, regex: false };
const CS = { caseSensitive: true, regex: false };
const RE = { caseSensitive: false, regex: true };
const RE_CS = { caseSensitive: true, regex: true };

describe("procSearch", () => {
  it("默认忽略大小写，开启后区分", () => {
    expect(countMatches("Foo foo FOO", compileSearch("foo", CI))).toBe(3);
    expect(countMatches("Foo foo FOO", compileSearch("foo", CS))).toBe(1);
  });

  it("普通模式把正则元字符当字面量", () => {
    const line = "a.b axb";
    expect(countMatches(line, compileSearch("a.b", CI))).toBe(1);
    expect(countMatches(line, compileSearch("a.b", RE))).toBe(2);
  });

  it("正则模式支持字符类与分组", () => {
    const line = "12c00000-22c00000 rw-p\n0xdeadbeef";
    expect(countMatches(line, compileSearch("0x[0-9a-f]{8}", RE))).toBe(1);
    // 高亮是按行调用 countMatches 的，所以 `^` 在每行都算行首
    expect(countMatches("12c00000-22c00000 rw-p", compileSearch("^\\S+", RE))).toBe(1);
    // 不带 m 标志时多行输入只有整段开头算行首
    expect(countMatches(line, compileSearch("^\\S+", RE))).toBe(1);
    // 区分大小写时 0xDEADBEEF 不命中
    expect(countMatches("0xDEADBEEF", compileSearch("0x[0-9a-f]{8}", RE_CS))).toBe(0);
  });

  it("非法正则不当成异常，只报无效", () => {
    expect(compileSearch("a([", RE)).toBeNull();
    expect(isInvalidQuery("a([", RE)).toBe(true);
    expect(isInvalidQuery("a([", CI)).toBe(false);
    expect(isInvalidQuery("", RE)).toBe(false);
  });

  it("空查询不产生任何高亮", () => {
    expect(compileSearch("", CI)).toBeNull();
    expect(countMatches("anything", null)).toBe(0);
    expect(splitLine("anything", null)).toEqual([{ text: "anything", hit: false }]);
  });

  it("0 宽匹配不产生高亮区间，且不会死循环", () => {
    expect(findRanges("abc", compileSearch("\\b", RE))).toEqual([]);
    expect(countMatches("abc", compileSearch("x*", RE))).toBe(0);
    expect(splitLine("abc", compileSearch("^", RE))).toEqual([
      { text: "abc", hit: false },
    ]);
  });

  it("splitLine 切出命中与非命中片段", () => {
    const segments = splitLine("libfoo.so libbar.so", compileSearch("lib", CI));
    expect(segments).toEqual([
      { text: "lib", hit: true },
      { text: "foo.so ", hit: false },
      { text: "lib", hit: true },
      { text: "bar.so", hit: false },
    ]);
    // 拼回去必须等于原文（渲染不能吞字符）
    expect(segments.map((s) => s.text).join("")).toBe("libfoo.so libbar.so");
  });

  it("同一行多次命中按出现顺序返回", () => {
    expect(findRanges("a-a-a", compileSearch("a", CI))).toEqual([
      [0, 1],
      [2, 3],
      [4, 5],
    ]);
  });
});
