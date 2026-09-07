import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AppProviders, useSettings } from "@/app/providers";

function Probe() {
  const { theme, effectiveTheme, opacity } = useSettings();
  return (
    <div
      data-testid="probe"
      data-theme={theme}
      data-effective={effectiveTheme}
      data-opacity={opacity}
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

  it("默认跟随系统且不透明度为 100", () => {
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    const probe = screen.getByTestId("probe");
    expect(probe).toHaveAttribute("data-theme", "system");
    expect(probe).toHaveAttribute("data-opacity", "100");
  });

  it("切到深色后 documentElement 挂 dark 类并持久化", () => {
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

  it("透明度被钳制在最小值之上并持久化", () => {
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

  it("恢复已持久化的主题选择", () => {
    localStorage.setItem("app.settings.theme", "light");
    render(
      <AppProviders>
        <Probe />
      </AppProviders>,
    );
    expect(screen.getByTestId("probe")).toHaveAttribute("data-theme", "light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});
