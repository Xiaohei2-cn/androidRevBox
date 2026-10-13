/**
 * frida 回调对象字面量的位置识别（fridaCallbacks.ts）。
 *
 * 这份测试钉的是"**什么时候该给 onEnter / onLeave**"——判错的两种后果都不好：
 * - 该给不给 → 用户回到"没提示"（就是这次要修的问题）；
 * - 不该给却给 → 在无关位置冒出两个错误建议，比没有更糟。
 *
 * 而 TS 语言服务在 union 参数上**永远不给**（实测：单一接口给、union 不给，
 * 与成员类型是否冲突无关），所以这段判断是本工具自己承担的责任。
 */
import { describe, expect, it } from "vitest";

import { detectCallbackContext } from "./fridaCallbacks";

describe("识别 Interceptor.attach 的配置对象位置", () => {
  it("`Interceptor.attach(addr, {` 之后（含换行与缩进）都识别为 attach 的配置对象", () => {
    for (const prefix of [
      "Interceptor.attach(addr, {",
      "Interceptor.attach(addr, {  ",
      "Interceptor.attach(addr, {\n  ",
      "  Interceptor.attach(module_base.add(0x1), {\n    ",
      "Interceptor.attach(ptr('0x1'),\n  {\n  ",
    ]) {
      const ctx = detectCallbackContext(prefix);
      expect(ctx, prefix).not.toBeNull();
      expect(ctx!.member, prefix).toBe("attach");
    }
  });

  it("已经敲了一半的字段名也算（用户正在输入 onEn 时）", () => {
    const ctx = detectCallbackContext("Interceptor.attach(addr, {\n  onEn");
    expect(ctx?.member).toBe("attach");
  });

  /**
   * ⚠️ 多行 prefix 必须能识别——这是**实际发生过的 bug**。
   *
   * provider 最初只取"光标所在行"的文本，而用户最常见的形状是
   * `Interceptor.attach(addr, {` 之后**回车换行**再写字段：那时当前行只有空格，
   * 判断不出上下文 → 永远不提示。所以必须喂**从文件开头到光标**的文本。
   */
  it("多行 prefix（用户按回车换行后书写）也要识别", () => {
    for (const prefix of [
      "var tf = ptr('0x1000');\nInterceptor.attach(tf, {\n  ",
      "Interceptor.attach(tf,\n  {\n  ",
      "const a = 1;\nInterceptor.attach(\n  tf,\n  {\n  ",
    ]) {
      expect(detectCallbackContext(prefix)?.member, JSON.stringify(prefix)).toBe("attach");
    }
  });

  it("其它 union 回调 API 同样识别（replaceFast）", () => {
    expect(detectCallbackContext("Interceptor.replaceFast(addr, {\n")?.member).toBe("replaceFast");
  });
});

describe("不该给提示的位置必须返回 null", () => {
  it("不在任何对象字面量里", () => {
    for (const prefix of ["", "Interceptor.", "Interceptor.attach(", "var x = 1;", "  "]) {
      expect(detectCallbackContext(prefix), JSON.stringify(prefix)).toBeNull();
    }
  });

  it("对象字面量但不是函数实参（如直接赋值）", () => {
    // `{` 之前是 `=` 而不是 `(`
    expect(detectCallbackContext("var opts = {")).toBeNull();
    expect(detectCallbackContext("Interceptor.attach(addr, x ? {")).toBeNull();
  });

  it("实参对象已经闭合（光标在 } 之后）", () => {
    // 对象字面量已闭合 + 右括号：此时不在任何字面量里
    expect(detectCallbackContext("Interceptor.attach(addr, {});")).toBeNull();
    expect(detectCallbackContext("Interceptor.attach(addr, { onEnter() {} });")).toBeNull();
  });

  it("在字面量内部即使已有闭合的方法体，仍算命中（真实书写过程中的位置）", () => {
    // `onEnter() {}` 的 `{}` 是配平的，外层 `{` 还没闭 —— 用户就在这里继续写 onLeave
    const ctx = detectCallbackContext("Interceptor.attach(addr, {\n  onEnter() {},\n  ");
    expect(ctx?.member).toBe("attach");
  });

  it("嵌套对象里：内层 `{` 是最近的未配对括号，成员名会取错 → 不匹配任何 spec", () => {
    // 这才是"内层不该提示外层字段"的保证：识别到的成员名不是 attach
    const ctx = detectCallbackContext("Interceptor.attach(addr, {\n  x: {\n    ");
    // 要么是 null，要么成员名不是 attach —— 两种情况都不会给出 onEnter
    expect(ctx === null || ctx.member !== "attach").toBe(true);
  });
});
