import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeCommand = vi.hoisted(() => vi.fn());

vi.mock("./client", () => ({ invokeCommand }));

import { zygiskApi } from "./zygisk";

describe("zygiskApi", () => {
  beforeEach(() => invokeCommand.mockReset());

  it("reads module lifecycle through the agent-backed status command", async () => {
    invokeCommand.mockResolvedValue({ lifecycle: "bridge_ready", bridgeReady: true });

    await expect(zygiskApi.status("serial-1")).resolves.toMatchObject({
      lifecycle: "bridge_ready",
    });
    expect(invokeCommand).toHaveBeenCalledWith("zygisk_status", { serial: "serial-1" });
  });

  it("sends scope plus explicit locale opt-ins to package.list_localized", async () => {
    invokeCommand.mockResolvedValue({ items: [], successCount: 0, fallbackCount: 0, warnings: [] });

    await zygiskApi.list("serial-1", "user");
    await zygiskApi.list("serial-1", "all", { locale: "zh-CN", includeDisabled: true });

    expect(invokeCommand.mock.calls).toEqual([
      [
        "package_list_localized",
        { serial: "serial-1", scope: "user", locale: null, includeDisabled: false },
      ],
      [
        "package_list_localized",
        { serial: "serial-1", scope: "all", locale: "zh-CN", includeDisabled: true },
      ],
    ]);
  });

  it("exports base and split APKs through the agent staging command", async () => {
    const report = {
      packageName: "com.example.app",
      kind: "apk",
      fileName: "示例应用_1.0.apk",
      artifactPath: "/tmp/apks/示例应用_1.0.apk",
      artifactBytes: 10,
      parts: [{ packageName: "com.example.app", name: "base.apk", size: 10 }],
      bytes: 10,
      appLabel: "示例应用",
      versionName: "1.0",
      nameSource: "zygisk_framework",
      skipped: [],
      complete: true,
      destination: "/tmp/apks",
    };
    invokeCommand.mockResolvedValue(report);

    await expect(
      zygiskApi.exportPackage("serial-1", "com.example.app", "/tmp/apks"),
    ).resolves.toEqual(report);
    expect(invokeCommand).toHaveBeenCalledWith("package_export_apk", {
      serial: "serial-1",
      packageName: "com.example.app",
      destination: "/tmp/apks",
    });
  });
});
