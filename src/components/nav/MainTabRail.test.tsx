import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AppNavProvider } from "@/app/nav";
import { DICTIONARIES } from "@/i18n/dictionaries";
import { I18nProvider } from "@/i18n/I18nProvider";
import { MAIN_TABS, MainTabRail } from "./MainTabRail";

const ZH_LABELS = DICTIONARIES["zh-CN"];

function renderRail() {
  return render(
    <TooltipProvider>
      <AppNavProvider>
        <I18nProvider>
          <MainTabRail />
        </I18nProvider>
      </AppNavProvider>
    </TooltipProvider>,
  );
}

describe("MainTabRail", () => {
  it("渲染全部 7 个总 tab（默认 zh-CN 标签）", () => {
    renderRail();
    for (const tab of MAIN_TABS) {
      expect(screen.getByRole("button", { name: ZH_LABELS[tab.labelKey] })).toBeInTheDocument();
    }
    expect(MAIN_TABS).toHaveLength(7);
  });

  it("点击 tab 切换激活态（context 驱动）", async () => {
    renderRail();
    // 初始激活：仪表盘（Provider 默认 tab）
    expect(screen.getByRole("button", { name: "仪表盘" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await userEvent.click(screen.getByRole("button", { name: "设备" }));
    const devices = screen.getByRole("button", { name: "设备" });
    expect(devices).toHaveAttribute("aria-current", "true");
    expect(devices).toHaveTextContent("设备");
    expect(screen.getByRole("button", { name: "仪表盘" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("当前激活 tab 标记 aria-current", () => {
    renderRail();
    const dashboard = screen.getByRole("button", { name: "仪表盘" });
    expect(dashboard).toHaveAttribute("aria-current", "true");
    const settings = screen.getByRole("button", { name: "设置" });
    expect(settings).not.toHaveAttribute("aria-current");
  });
});
