import { describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { hookApi } from "@/api/hook";

/**
 * Hook API 的入参形状护栏。
 *
 * parseFridaLine / summarizeEventData 的测试随结构化终端一起退役（第九十六轮）：
 * NDJSON 行协议、三轨过滤、导出拼装都不再存在，本文件只管**还在的命令**怎么发。
 */

describe("hook_preflight 的入参", () => {
  it("USB 模式必须把 serial 一起送出去（否则探的是上一台机的 frida-server）", async () => {
    invokeMock.mockReset().mockResolvedValue({});
    await hookApi.preflight(undefined, "18271FDF600FL4");
    expect(invokeMock).toHaveBeenCalledWith("hook_preflight", {
      remote: null,
      serial: "18271FDF600FL4",
    });
  });

  it("远程模式给 endpoint，serial 留空", async () => {
    invokeMock.mockReset().mockResolvedValue({});
    await hookApi.preflight("127.0.0.1:27042");
    expect(invokeMock).toHaveBeenCalledWith("hook_preflight", {
      remote: "127.0.0.1:27042",
      serial: null,
    });
  });
});

describe("脚本读写接口（IDE 用的三条）", () => {
  it("读：只传名字，边界在宿主侧", async () => {
    invokeMock.mockReset().mockResolvedValue("// x");
    await hookApi.scriptRead("hook.js");
    expect(invokeMock).toHaveBeenCalledWith("hook_script_read", { name: "hook.js" });
  });

  it("写：默认不是 createOnly；新建时显式为 true（宿主据此拒绝撞名）", async () => {
    invokeMock.mockReset().mockResolvedValue(3);
    await hookApi.scriptWrite("a.js", "abc");
    expect(invokeMock).toHaveBeenLastCalledWith("hook_script_write", {
      name: "a.js",
      content: "abc",
      createOnly: false,
    });
    await hookApi.scriptWrite("b.js", "x", true);
    expect(invokeMock).toHaveBeenLastCalledWith("hook_script_write", {
      name: "b.js",
      content: "x",
      createOnly: true,
    });
  });

  it("删：只传名字", async () => {
    invokeMock.mockReset().mockResolvedValue(undefined);
    await hookApi.scriptDelete("gone.js");
    expect(invokeMock).toHaveBeenCalledWith("hook_script_delete", { name: "gone.js" });
  });
});