import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AppProviders, useSettings } from "@/app/providers";

function Probe() {
  const { theme, effectiveTheme, opacity, logLevel, hydrated } = useSettings();
  return (
    <div
      data-testid="probe"
      data-theme={theme}
      data-effective={effectiveTheme}
      data-opacity={opacity}
      data-log-level={logLevel}
      data-hydrated={String(hydrated)}
    />
  );
}

describe("SettingsProvider", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.className = "";
  });

  afterEach(() => {
    localStorage.clear();
  });

  it("默认跟随系统、不透明度 100、日志 info，纯浏览器环境直接完成水合", () => {
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    const probe = screen.getByTestId("probe");
    expect(probe).toHaveAttribute("data-theme", "system");
    expect(probe).toHaveAttribute("data-opacity", "100");
    expect(probe).toHaveAttribute("data-log-level", "info");
    expect(probe).toHaveAttribute("data-hydrated", "true");
  });

  it("切到深色后 documentElement 挂 dark 类并写本地缓存", () => {
    function Setter() {
      const { setTheme } = useSettings();
      return (
        <button type="button" onClick={() => setTheme("dark")}>
          set
        </button>
      );
    }
    render(
      <AppProviders>
        <Probe />
        <Setter />
      </AppProviders>,
    );
    act(() => {
      screen.getByRole("button").click();
    });
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(localStorage.getItem("app.settings.theme")).toBe("dark");
    expect(screen.getByTestId("probe")).toHaveAttribute("data-effective", "dark");
  });

  it("透明度被钳制在最小值之上并写本地缓存", () => {
    function Setter() {
      const { setOpacity } = useSettings();
      return (
        <button type="button" onClick={() => setOpacity(0)}>
          set
        </button>
      );
    }
    render(
      <AppProviders>
        <Probe />
        <Setter />
      </AppProviders>,
    );
    act(() => {
      screen.getByRole("button").click();
    });
    expect(screen.getByTestId("probe")).toHaveAttribute("data-opacity", "20");
    expect(localStorage.getItem("app.settings.opacity")).toBe("20");
  });

  it("P0 的 localStorage 旧值直接作为初值恢复（迁移目标场景）", () => {
    localStorage.setItem("app.settings.theme", "light");
    localStorage.setItem("app.settings.opacity", "70");
    localStorage.setItem("app.settings.log_level", "debug");
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    const probe = screen.getByTestId("probe");
    expect(probe).toHaveAttribute("data-theme", "light");
    expect(probe).toHaveAttribute("data-opacity", "70");
    expect(probe).toHaveAttribute("data-log-level", "debug");
  });

  it("非法缓存值被忽略，使用默认值", () => {
    localStorage.setItem("app.settings.theme", "neon");
    localStorage.setItem("app.settings.opacity", "abc");
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    const probe = screen.getByTestId("probe");
    expect(probe).toHaveAttribute("data-theme", "system");
    expect(probe).toHaveAttribute("data-opacity", "100");
  });

  it("日志级别 setter 生效并缓存", () => {
    function Setter() {
      const { setLogLevel } = useSettings();
      return (
        <button type="button" onClick={() => setLogLevel("warn")}>
          set
        </button>
      );
    }
    render(
      <AppProviders>
        <Probe />
        <Setter />
      </AppProviders>,
    );
    act(() => {
      screen.getByRole("button").click();
    });
    expect(screen.getByTestId("probe")).toHaveAttribute("data-log-level", "warn");
    expect(localStorage.getItem("app.settings.log_level")).toBe("warn");
  });
});
