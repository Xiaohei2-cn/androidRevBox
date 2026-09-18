import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeCommand = vi.hoisted(() => vi.fn());

vi.mock("./client", () => ({ invokeCommand }));

import { agentApi } from "./agent";

describe("agentApi", () => {
  beforeEach(() => invokeCommand.mockReset());

  it("uses stable command names and serial arguments", async () => {
    invokeCommand.mockResolvedValue({ state: "ready" });
    await agentApi.status("serial-1");
    await agentApi.install("serial-1");
    await agentApi.restart("serial-1");
    await agentApi.diagnostics("serial-1");

    expect(invokeCommand.mock.calls).toEqual([
      ["agent_status", { serial: "serial-1" }],
      ["agent_install", { serial: "serial-1" }],
      ["agent_restart", { serial: "serial-1" }],
      ["agent_diagnostics", { serial: "serial-1" }],
    ]);
  });

  it("requests all tracked device statuses without arguments", async () => {
    invokeCommand.mockResolvedValue([]);
    await expect(agentApi.statuses()).resolves.toEqual([]);
    expect(invokeCommand).toHaveBeenCalledWith("agent_statuses");
  });
});
