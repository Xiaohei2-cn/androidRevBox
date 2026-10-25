/**
 * 「新建脚本」的名字口径：默认只落 `.js`。
 *
 * 这条纯函数被两个入口共用（B 区脚本条 / 编辑器工具条），所以它自己的边界单独钉一遍——
 * 别等到界面上才发现"补后缀"补出了 `hook.txt.js` 这种既不是她敲的、也不是 .js 的东西。
 */
import { describe, expect, it } from "vitest";

import { ensureJsName } from "./newScript";

describe("ensureJsName：只给名字就补 .js，别的扩展名不猜", () => {
  it("没有扩展名：补 .js", () => {
    expect(ensureJsName("hook_test")).toBe("hook_test.js");
  });

  it("已经是 .js：原样（前后空白去掉）", () => {
    expect(ensureJsName("  hook_test.js  ")).toBe("hook_test.js");
  });

  it("带点的名字：只有完全没写扩展名才补 .js（hook.v2 交出去拒绝，不猜成 hook.v2.js）", () => {
    expect(ensureJsName("hook.v2")).toBe("hook.v2");
    expect(ensureJsName("hook.v2.js")).toBe("hook.v2.js");
  });

  it("别的扩展名原样交出去（由 isSafeJsName 拒绝，不静默改名成 .txt.js）", () => {
    expect(ensureJsName("hook.txt")).toBe("hook.txt");
    expect(ensureJsName("hook.py")).toBe("hook.py");
  });

  it("末尾多余的点剥掉，不造出 foo..js", () => {
    expect(ensureJsName("foo.")).toBe("foo.js");
    expect(ensureJsName("foo.js.")).toBe("foo.js");
  });

  it("空输入还是空（不补出一个 .js 来）", () => {
    expect(ensureJsName("")).toBe("");
    expect(ensureJsName("   ")).toBe("");
  });
});
