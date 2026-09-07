import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { SettingsPage } from "./SettingsPage";

describe("SettingsPage", () => {
  it("渲染三个主题选项", () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    for (const label of ["浅色", "深色", "跟随系统"]) {
      expect(screen.getByRole("radio", { name: label })).toBeInTheDocument();
    }
  });

  it("点击主题选项后 radiogroup 选中态更新", async () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    const dark = screen.getByRole("radio", { name: "深色" });
    await userEvent.click(dark);
    expect(dark).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: "跟随系统" }),
    ).toHaveAttribute("aria-checked", "false");
  });

  it("透明度滑杆存在且初始值为 100", () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    expect(screen.getByRole("slider")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });
});
