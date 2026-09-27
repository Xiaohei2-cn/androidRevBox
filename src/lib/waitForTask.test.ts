import { beforeEach, describe, expect, it, vi } from "vitest";

const list = vi.hoisted(() => vi.fn());
vi.mock("@/api/task", () => ({ taskApi: { list } }));

import { waitForTask } from "./waitForTask";

const task = (status: string, id = "t1"): unknown => ({
  id,
  taskType: "adb.push",
  name: "adb push",
  status,
  exitCode: status === "success" ? 0 : 1,
  createdAt: 0,
  finishedAt: 1,
});

describe("waitForTask", () => {
  beforeEach(() => list.mockReset());

  it("跑到终态就返回那个状态，不再轮询", async () => {
    list.mockResolvedValue([task("success")]);
    await expect(waitForTask("t1", { pollMs: 1, timeoutMs: 200 })).resolves.toBe("success");
    expect(list).toHaveBeenCalledTimes(1);
  });

  it("pending/running 继续等，直到出结果", async () => {
    list
      .mockResolvedValueOnce([task("pending")])
      .mockResolvedValueOnce([task("running")])
      .mockResolvedValueOnce([task("failed")]);
    await expect(waitForTask("t1", { pollMs: 1, timeoutMs: 500 })).resolves.toBe("failed");
    expect(list).toHaveBeenCalledTimes(3);
  });

  it("IPC 抖一下不能当成失败：吞掉这一次，下轮再问", async () => {
    list.mockRejectedValueOnce(new Error("boom")).mockResolvedValueOnce([task("success")]);
    await expect(waitForTask("t1", { pollMs: 1, timeoutMs: 500 })).resolves.toBe("success");
  });

  it("一直查不到这个任务 → unknown（不猜成功，也不猜失败）", async () => {
    list.mockResolvedValue([task("success", "别人")]);
    await expect(waitForTask("t1", { pollMs: 1, timeoutMs: 20 })).resolves.toBe("unknown");
  });

  it("还在跑但已超时 → timeout，让界面照实说", async () => {
    list.mockResolvedValue([task("running")]);
    await expect(waitForTask("t1", { pollMs: 1, timeoutMs: 20 })).resolves.toBe("timeout");
  });
});
