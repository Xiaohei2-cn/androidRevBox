import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// 必须在导入被测模块之前完成 mock 定义
const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

import { AppProviders, useSettings } from "@/app/providers";

function Probe() {
  const { theme, opacity, logLevel, hydrated } = useSettings();
  return (
    <div
      data-testid="probe"
      data-theme={theme}
      data-opacity={opacity}
      data-log-level={logLevel}
      data-hydrated={String(hydrated)}
    />
  );
}

describe("SettingsProvider（Tauri 环境：SQLite 持久化 + P0 迁移）", () => {
  beforeEach(() => {
    localStorage.clear();
    invokeMock.mockReset();
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  });

  afterEach(() => {
    localStorage.clear();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("DB 值覆盖 localStorage 缓存（SQLite 为事实源）", async () => {
    localStorage.setItem("app.settings.theme", "light");
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "config_snapshot") {
        return [{ key: "app.settings.theme", value: "dark" }];
      }
      return null;
    });
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    // 缓存初值先可见
    expect(screen.getByTestId("probe")).toHaveAttribute("data-theme", "light");
    await waitFor(() =>
      expect(screen.getByTestId("probe")).toHaveAttribute("data-hydrated", "true"),
    );
    expect(screen.getByTestId("probe")).toHaveAttribute("data-theme", "dark");
    // 水合后缓存也被对齐
    expect(localStorage.getItem("app.settings.theme")).toBe("dark");
  });

  it("P0 旧值一次性迁移：DB 缺失的键会被回写", async () => {
    localStorage.setItem("app.settings.theme", "dark");
    localStorage.setItem("app.settings.opacity", "60");
    localStorage.setItem("app.settings.log_level", "warn");
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "config_snapshot") return [];
      if (cmd === "config_set") return null;
      if (cmd === "log_set_level") return (args as { level: string }).level;
      return null;
    });
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("probe")).toHaveAttribute("data-hydrated", "true"),
    );
    const setCalls = invokeMock.mock.calls.filter((c) => c[0] === "config_set");
    const keys = setCalls.map((c) => (c[1] as { args: { key: string } }).args.key);
    expect(keys).toEqual(
      expect.arrayContaining(["app.settings.theme", "app.settings.opacity"]),
    );
    expect(invokeMock).toHaveBeenCalledWith("log_set_level", { level: "warn" });
  });

  it("水合完成后修改设置才写回 DB，且日志级别走 log_set_level", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "config_snapshot") return [];
      return null;
    });
    function Setter() {
      const { setTheme, setLogLevel } = useSettings();
      return (
        <>
          <button type="button" onClick={() => setTheme("dark")}>
            theme
          </button>
          <button type="button" onClick={() => setLogLevel("error")}>
            level
          </button>
        </>
      );
    }
    render(
      <AppProviders>
        <Probe />
        <Setter />
      </AppProviders>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("probe")).toHaveAttribute("data-hydrated", "true"),
    );
    invokeMock.mockClear();
    act(() => {
      screen.getByRole("button", { name: "theme" }).click();
    });
    act(() => {
      screen.getByRole("button", { name: "level" }).click();
    });
    expect(invokeMock).toHaveBeenCalledWith("config_set", {
      args: { key: "app.settings.theme", value: "dark" },
    });
    expect(invokeMock).toHaveBeenCalledWith("log_set_level", { level: "error" });
  });

  it("后端 snapshot 失败时保留缓存值并继续水合", async () => {
    localStorage.setItem("app.settings.theme", "light");
    invokeMock.mockRejectedValue({ code: "DATABASE", message: "库坏了" });
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("probe")).toHaveAttribute("data-hydrated", "true"),
    );
    expect(screen.getByTestId("probe")).toHaveAttribute("data-theme", "light");
    spy.mockRestore();
  });
});
