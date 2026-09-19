import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeCommand = vi.hoisted(() => vi.fn());

vi.mock("./client", () => ({ invokeCommand }));

import { zygiskApi } from "./zygisk";

describe("zygiskApi", () => {
  beforeEach(() => invokeCommand.mockReset());

  it("uses the stable list command and serial argument", async () => {
    invokeCommand.mockResolvedValue([]);

    await expect(zygiskApi.list("serial-1")).resolves.toEqual([]);
    expect(invokeCommand).toHaveBeenCalledWith("zygisk_applist", { serial: "serial-1" });
  });

  it("reads the per-package APK manifest through the E command", async () => {
    invokeCommand.mockResolvedValue({ "com.example.app": [] });

    await expect(zygiskApi.manifest("serial-1")).resolves.toEqual({
      "com.example.app": [],
    });
    expect(invokeCommand).toHaveBeenCalledWith("zygisk_apk_manifest", { serial: "serial-1" });
  });

  it("passes package and destination to the streaming export command", async () => {
    const report = {
      packageName: "com.example.app",
      files: [],
      destination: "/tmp/apks/com.example.app",
      bytes: 0,
    };
    invokeCommand.mockResolvedValue(report);

    await expect(
      zygiskApi.exportPackage("serial-1", "com.example.app", "/tmp/apks"),
    ).resolves.toEqual(report);
    expect(invokeCommand).toHaveBeenCalledWith("zygisk_applist_export", {
      serial: "serial-1",
      packageName: "com.example.app",
      destination: "/tmp/apks",
    });
  });
});
