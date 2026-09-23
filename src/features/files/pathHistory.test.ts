import { describe, expect, it } from "vitest";
import {
  canGoBack,
  canGoForward,
  currentPath,
  goBack,
  goForward,
  goTo,
  initialHistory,
  normalizeRemotePath,
  parentOf,
} from "./pathHistory";

/**
 * 文件页浏览历史的规则（纯函数）。界面上那三个按钮的行为全部由这里决定，
 * 所以值得单独钉住：尤其是"去了新地方就丢掉前进分支"这条浏览器习惯，
 * 以及"到根时不给假路径"。
 */
describe("远端路径归一化", () => {
  it("补齐前导斜杠、折叠重复斜杠、去掉尾部斜杠", () => {
    expect(normalizeRemotePath("sdcard/Download")).toBe("/sdcard/Download");
    expect(normalizeRemotePath("//sdcard///Download/")).toBe("/sdcard/Download");
    expect(normalizeRemotePath("/")).toBe("/");
    expect(normalizeRemotePath("   ")).toBe("");
  });

  it("就地展开 . 与 ..，不留 /.. 这种能逃出根的路径", () => {
    expect(normalizeRemotePath("/sdcard/./Download")).toBe("/sdcard/Download");
    expect(normalizeRemotePath("/sdcard/Download/../Music")).toBe("/sdcard/Music");
    expect(normalizeRemotePath("/../../etc")).toBe("/etc");
  });
});

describe("上一级", () => {
  it("逐级回退，根目录返回 null", () => {
    expect(parentOf("/sdcard/Download")).toBe("/sdcard");
    expect(parentOf("/sdcard")).toBe("/");
    expect(parentOf("/")).toBeNull();
    // 手输的脏路径也要给出同一个答案，不能拼出 "//"
    expect(parentOf("/sdcard/Download/")).toBe("/sdcard");
  });
});

describe("浏览历史", () => {
  it("同一条路径不产生历史记录", () => {
    const once = initialHistory("/sdcard");
    expect(goTo(once, "/sdcard")).toBe(once);
    expect(canGoBack(once)).toBe(false);
    expect(canGoForward(once)).toBe(false);
  });

  it("后退与前进沿着走过的路径来回，且都能停在原地", () => {
    let h = initialHistory("/sdcard");
    h = goTo(h, "/sdcard/Download");
    h = goTo(h, "/data/local/tmp");
    expect(currentPath(h)).toBe("/data/local/tmp");
    h = goBack(h);
    expect(currentPath(h)).toBe("/sdcard/Download");
    h = goBack(h);
    expect(currentPath(h)).toBe("/sdcard");
    expect(goBack(h)).toEqual(h); // 已经到底，不越界
    h = goForward(h);
    expect(currentPath(h)).toBe("/sdcard/Download");
    h = goForward(h);
    expect(currentPath(h)).toBe("/data/local/tmp");
    expect(goForward(h)).toEqual(h); // 已经到头
  });

  it("在历史记录中间去新地方时，丢掉后面的前进分支（与浏览器一致）", () => {
    let h = initialHistory("/sdcard");
    h = goTo(h, "/a");
    h = goTo(h, "/b");
    h = goBack(h); // 现在在 /a
    h = goBack(h); // 现在在 /sdcard
    h = goTo(h, "/c");
    expect(currentPath(h)).toBe("/c");
    expect(canGoForward(h)).toBe(false);
    // 前进分支被截断：后退只能回到 /sdcard，不会看见 /a /b
    h = goBack(h);
    expect(currentPath(h)).toBe("/sdcard");
    expect(canGoBack(h)).toBe(false);
  });

  it("归一化过的同一路径算同一个位置", () => {
    let h = goTo(initialHistory("/sdcard"), "/sdcard/");
    h = goTo(h, "//sdcard");
    expect(h.entries).toEqual(["/sdcard"]);
  });
});
