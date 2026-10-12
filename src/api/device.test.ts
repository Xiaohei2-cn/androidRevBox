import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeCommand = vi.hoisted(() => vi.fn());

vi.mock("./client", () => ({ invokeCommand }));

import { deviceApi } from "./device";

/**
 * AR7.4 契约护栏：设备/文件/托管这一片的能力已经迁到 Agent typed API，
 * 但 command 名与入参形状是前端与后端的稳定接口（阶段文档 AR7.4 明确要求
 * 「command 名保持不变」）。这里把名字和 payload 钉死，改动必须是有意的。
 */
describe("deviceApi command contract", () => {
  beforeEach(() => {
    invokeCommand.mockReset();
    invokeCommand.mockResolvedValue([]);
  });

  it("keeps the file browsing commands stable (AR7.1)", async () => {
    await deviceApi.ls("serial-1", "/sdcard");
    await deviceApi.fileStat("serial-1", "/sdcard/a");
    await deviceApi.filePreview("serial-1", "/sdcard/a", { fromEnd: true });

    expect(invokeCommand.mock.calls).toEqual([
      ["device_ls", { serial: "serial-1", path: "/sdcard", root: false }],
      [
        "device_file_stat",
        { serial: "serial-1", path: "/sdcard/a", followSymlink: false, root: false },
      ],
      [
        "device_file_preview",
        {
          serial: "serial-1",
          path: "/sdcard/a",
          maxBytes: null,
          fromEnd: true,
          root: false,
        },
      ],
    ]);
  });

  it("carries the root identity through every read command (2026-10-03)", async () => {
    // 读也要带身份：`/data` 这类目录 shell 连 opendir 都进不去，不带 root 就永远只能看到
    // "读不到"或（更糟）一个空数组被画成"空目录"。
    await deviceApi.ls("serial-1", "/data", true);
    await deviceApi.fileStat("serial-1", "/data/system/vold.fstab", false, true);
    await deviceApi.filePreview("serial-1", "/data/local/tmp/x.log", { root: true });

    expect(invokeCommand.mock.calls).toEqual([
      ["device_ls", { serial: "serial-1", path: "/data", root: true }],
      ["device_file_stat", { serial: "serial-1", path: "/data/system/vold.fstab", followSymlink: false, root: true }],
      [
        "device_file_preview",
        { serial: "serial-1", path: "/data/local/tmp/x.log", maxBytes: null, fromEnd: false, root: true },
      ],
    ]);
  });

  it("keeps the hosted binary commands stable (AR7.2/AR7.3)", async () => {
    await deviceApi.binaries("serial-1");
    await deviceApi.binaryChmod("serial-1", "toybox", true);
    await deviceApi.binaryRun("serial-1", "toybox", false);
    await deviceApi.hostedRuns("serial-1");
    await deviceApi.hostedStop("serial-1", "aabbccdd00112233", 4242);
    await deviceApi.binaryPorts("serial-1", 4242, false);

    expect(invokeCommand.mock.calls).toEqual([
      ["device_binaries", { serial: "serial-1" }],
      ["device_binary_chmod", { serial: "serial-1", name: "toybox", root: true }],
      [
        "device_binary_run",
        {
          serial: "serial-1",
          name: "toybox",
          root: false,
          args: [],
        },
      ],
      ["device_hosted_runs", { serial: "serial-1" }],
      [
        "device_hosted_stop",
        { serial: "serial-1", handle: "aabbccdd00112233", expectedPid: 4242 },
      ],
      ["device_binary_ports", { serial: "serial-1", pid: 4242, root: false }],
    ]);
  });

  it("claims the terminal-launched process with the pre-launch pids (AR7.7)", async () => {
    // 交互终端那次 exec 不经过 Agent，运行表要靠这条补登记；
    // 已知 pid 必须一起交出去：那些不是这次起的，不能被认领成自己的。
    await deviceApi.binaryClaim("serial-1", "toybox", ["-p", "2345"], [31337], true);
    await deviceApi.binaryClaim("serial-1", "toybox");
    expect(invokeCommand.mock.calls).toEqual([
      [
        "device_binary_claim",
        {
          serial: "serial-1",
          name: "toybox",
          args: ["-p", "2345"],
          knownPids: [31337],
          root: true,
        },
      ],
      [
        "device_binary_claim",
        { serial: "serial-1", name: "toybox", args: [], knownPids: [], root: false },
      ],
    ]);
  });

  /// UI-6 第一/二/四层的 IPC 形状。**启动不再带 stdinData/interactive**：
  /// 一次性喂 stdin 那条路已经被交互终端替掉（喂完就 EOF，边跑边问的程序会卡死），
  /// 运行中的持续输入 hostedWrite 仍然是活的，所以它照旧按稳定键上线。
  it("carries launch args, probe and running input with stable keys", async () => {
    await deviceApi.binaryRun("serial-1", "toybox", true, ["a b", "c;id"]);
    await deviceApi.binaryRun("serial-1", "toybox");
    await deviceApi.binaryProbe("serial-1", "toybox", ["-h"], 4000);
    await deviceApi.hostedWrite("serial-1", "handle-1", "1\n", false);
    await deviceApi.hostedWrite("serial-1", "handle-1", "", true);

    expect(invokeCommand.mock.calls).toEqual([
      [
        "device_binary_run",
        {
          serial: "serial-1",
          name: "toybox",
          root: true,
          args: ["a b", "c;id"],
        },
      ],
      [
        "device_binary_run",
        { serial: "serial-1", name: "toybox", root: false, args: [] },
      ],
      ["device_binary_probe", { serial: "serial-1", name: "toybox", args: ["-h"], timeoutMs: 4000 }],
      ["device_hosted_write", { serial: "serial-1", handle: "handle-1", text: "1\n", close: false }],
      ["device_hosted_write", { serial: "serial-1", handle: "handle-1", text: "", close: true }],
    ]);
  });

  it("passes the displayed process name to kill so the device can verify identity", async () => {
    await deviceApi.binaryKill("serial-1", 4242, false, "toybox");
    await deviceApi.binaryKill("serial-1", 4243, true);

    expect(invokeCommand.mock.calls).toEqual([
      [
        "device_binary_kill",
        { serial: "serial-1", pid: 4242, root: false, expectedName: "toybox" },
      ],
      ["device_binary_kill", { serial: "serial-1", pid: 4243, root: true, expectedName: undefined }],
    ]);
  });

  it("keeps port cross-lookup and transport commands stable", async () => {
    await deviceApi.procPorts("serial-1", 4242);
    await deviceApi.procByPort("serial-1", 11500, true);
    await deviceApi.push("serial-1", "/tmp/a", "/data/local/tmp/a");
    await deviceApi.pull("serial-1", "/data/local/tmp/a", "/tmp/a");

    expect(invokeCommand.mock.calls).toEqual([
      ["device_proc_ports", { serial: "serial-1", pid: 4242, root: false }],
      ["device_proc_by_port", { serial: "serial-1", port: 11500, root: true }],
      [
        "device_push",
        { args: { serial: "serial-1", local: "/tmp/a", remote: "/data/local/tmp/a" } },
      ],
      [
        "device_pull",
        { args: { serial: "serial-1", remote: "/data/local/tmp/a", local: "/tmp/a" } },
      ],
    ]);
  });
});
