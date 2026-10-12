/**
 * 界面侧的"哪儿能改"名单必须与 Rust `services/fs_policy.rs` 同一份口径：
 * 这里只用来提前禁按钮并说清原因，真正拦人的在后端。两边不一致就是
 * "按钮能点却报错"或"看得见却改不动"，两种都难查。
 */
import { describe, expect, it } from "vitest";

import {
  WRITABLE_ROOTS,
  canWritePath,
  normalizeDevicePath,
  writeDenial,
} from "./deviceFsPolicy";

describe("deviceFsPolicy：增删改只在那两棵树里", () => {
  it("名单就是调试目录 + 外部存储（含 /sdcard 的真身）", () => {
    expect(WRITABLE_ROOTS).toEqual(["/data/local/tmp", "/sdcard", "/storage/emulated/0"]);
  });

  it("两种写法、折叠后的路径都算在同一棵树里", () => {
    for (const ok of [
      "/data/local/tmp",
      "/data/local/tmp/frida-server",
      "/sdcard",
      "/sdcard/Download/x.apk",
      "/storage/emulated/0/Download/x.apk",
      "/sdcard/Download/../x",
      "/data/local/tmp//a/./b",
    ]) {
      expect(writeDenial(ok), `${ok} 该允许`).toBeNull();
      expect(canWritePath(ok)).toBe(true);
    }
  });

  it("别处一律不给改（root 也不行）", () => {
    for (const bad of [
      "/data/data/com.android.providers",
      "/data/local/tmpX/payload", // 前缀像但不是那一棵树
      "/system/bin/sh",
      "/sdcard2/x",
      "/storage/emulated/100/x",
      "/",
      "relative/thing",
      "",
    ]) {
      expect(canWritePath(bad), `${bad} 不该被允许`).toBe(false);
    }
    expect(writeDenial("relative/thing")).toBe("not-absolute");
    expect(writeDenial("/sdcard/../../data/data/x")).toBe("escapes-root");
    expect(writeDenial("/system/bin/sh")).toBe("outside");
  });

  it("normalize 与 Rust 同一套折叠：越顶返回 null", () => {
    expect(normalizeDevicePath("/a//b/./c/")).toBe("/a/b/c");
    expect(normalizeDevicePath("/")).toBe("/");
    expect(normalizeDevicePath("/a/b/../../c")).toBe("/c");
    expect(normalizeDevicePath("/a/../../b")).toBeNull();
  });
});
