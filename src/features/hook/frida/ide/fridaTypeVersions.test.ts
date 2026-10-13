/**
 * frida 类型库的版本解析（智能提示的内容跟着选定环境的 frida 版本走）。
 *
 * 为什么要有这份测试：**提示的"内容对不对"全押在这张映射表上**。内置一份类型库
 * 只对某一个 frida 主线正确，用户换版本后若仍用内置版，给出的就是错的方法签名——
 * 那种错不会报红，只会让人按错误的 API 写脚本。
 *
 * 钉四件事：
 * 1. 已知主线 → 解析到对应版本（16→16.5.1、17→17.3.1…）；
 * 2. **frida ≤16 的 java-bridge 用内置**：实测该包 7.0.0 才发布 index.d.ts，
 *    16 线在 npm 上根本没有类型文件，不能假装"跟随了"；
 * 3. 未知主线 / 没探测到 → 回退内置，且说明里如实讲清；
 * 4. 版本号解析的边界（`16.5.7`、`16.5.7-rc1`、空、乱码）。
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

import {
  fridaMajor,
  typesVersionsFor,
  resolveFridaTypes,
} from "./fridaTypeVersions";

const BUNDLED = { gum: "/* bundled gum */", javaBridge: "/* bundled jb */" };

describe("frida 主线号解析", () => {
  it("正常版本号取主线", () => {
    expect(fridaMajor("16.5.7")).toBe(16);
    expect(fridaMajor("17.17.0")).toBe(17);
    expect(fridaMajor(" 19.12.0 ")).toBe(19);
    // 预发布后缀不影响主线
    expect(fridaMajor("16.5.7-rc1")).toBe(16);
  });

  it("空/乱码一律 null（不猜）", () => {
    for (const v of [null, undefined, "", "   ", "abc", "v16", "16"]) {
      expect(fridaMajor(v as string)).toBeNull();
    }
  });
});

describe("主线 → 类型库版本映射", () => {
  it("已知主线给出对应版本", () => {
    expect(typesVersionsFor("16.5.7")).toEqual({ gum: "16.5.1", javaBridge: null });
    expect(typesVersionsFor("17.17.0")).toEqual({ gum: "17.3.1", javaBridge: "7.0.13" });
    expect(typesVersionsFor("18.2.0")).toEqual({ gum: "18.8.2", javaBridge: "7.0.13" });
    expect(typesVersionsFor("19.0.0")).toEqual({ gum: "19.12.0", javaBridge: "7.0.13" });
  });

  it("未知主线返回 null（由调用方回退内置）", () => {
    expect(typesVersionsFor("99.1.2")).toBeNull();
    expect(typesVersionsFor(null)).toBeNull();
    expect(typesVersionsFor("")).toBeNull();
  });
});

describe("resolveFridaTypes", () => {
  beforeEach(() => {
    // 每个用例用干净的正文缓存（默认不走网络：把 fetch 换成"总是失败"）
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404 }) as unknown as Response));
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("拉取失败 → 回退内置，并如实说明（离线时提示不会消失）", async () => {
    const r = await resolveFridaTypes("17.17.0", BUNDLED, new Map());
    expect(r.gum.source).toBe("bundled");
    expect(r.gum.content).toBe(BUNDLED.gum);
    expect(r.javaBridge.source).toBe("bundled");
    expect(r.note).toContain("回退内置");
  });

  it("拉取成功 → 用环境对应版本，说明里带上具体版本号", async () => {
    const gumText = "/*".padEnd(1200, "x"); // 超过防呆下限（>1000 字符）
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, status: 200, text: async () => gumText }) as unknown as Response),
    );
    const r = await resolveFridaTypes("17.17.0", BUNDLED, new Map());
    expect(r.gum.source).toBe("env");
    expect(r.gum.content).toBe(gumText);
    expect(r.gum.label).toBe("@types/frida-gum@17.3.1");
    expect(r.note).toContain("17.3.1");
    expect(r.note).toContain("17.17.0");
  });

  it("frida ≤16：gum 跟随、java-bridge 内置，说明讲清「该主线未发布类型」", async () => {
    const gumText = "/*".padEnd(1200, "x");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, status: 200, text: async () => gumText }) as unknown as Response),
    );
    const r = await resolveFridaTypes("16.5.7", BUNDLED, new Map());
    expect(r.gum.source).toBe("env");
    expect(r.gum.label).toBe("@types/frida-gum@16.5.1");
    expect(r.javaBridge.source).toBe("bundled");
    expect(r.note).toContain("未发布类型");
    // 不许假装跟随了
    expect(r.note).not.toContain("+ frida-java-bridge@");
  });

  it("拉到的正文太短（CDN 出错回一段 HTML）→ 当失败处理，不用垃圾覆盖类型库", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, status: 200, text: async () => "<html>404</html>" }) as unknown as Response),
    );
    const r = await resolveFridaTypes("17.17.0", BUNDLED, new Map());
    expect(r.gum.source).toBe("bundled");
    expect(r.gum.content).toBe(BUNDLED.gum);
  });

  it("同版本第二次解析走缓存（不再发请求）", async () => {
    const gumText = "/*".padEnd(1200, "x");
    const fetchMock = vi.fn(
      async () => ({ ok: true, status: 200, text: async () => gumText }) as unknown as Response,
    );
    vi.stubGlobal("fetch", fetchMock);
    const cache = new Map<string, string>();
    await resolveFridaTypes("17.17.0", BUNDLED, cache);
    const callsAfterFirst = fetchMock.mock.calls.length;
    await resolveFridaTypes("17.17.0", BUNDLED, cache);
    expect(fetchMock.mock.calls.length).toBe(callsAfterFirst); // 没有新增请求
  });

  it("未知主线 / 没探测到 → 内置 + 说明（不请求网络）", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const rWeird = await resolveFridaTypes("99.1.2", BUNDLED, new Map());
    expect(rWeird.gum.source).toBe("bundled");
    expect(rWeird.note).toContain("未知的 frida 主线");
    const rNone = await resolveFridaTypes(null, BUNDLED, new Map());
    expect(rNone.gum.source).toBe("bundled");
    expect(rNone.note).toContain("还没探测到");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});