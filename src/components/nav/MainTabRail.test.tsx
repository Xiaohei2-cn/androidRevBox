import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { MAIN_TABS, MainTabRail, type TabId } from "./MainTabRail";

function renderRail(props: React.ComponentProps<typeof MainTabRail>) {
  return render(
    <TooltipProvider>
      <MainTabRail {...props} />
    </TooltipProvider>,
  );
}

describe("MainTabRail", () => {
  it("渲染全部 7 个总 tab", () => {
    renderRail({ active: "dashboard", onChange: () => {} });
    for (const tab of MAIN_TABS) {
      expect(screen.getByRole("button", { name: tab.label })).toBeInTheDocument();
    }
    expect(MAIN_TABS).toHaveLength(7);
  });

  it("点击 tab 触发回调并携带正确 id", async () => {
    const onChange = vi.fn();
    renderRail({ active: "dashboard", onChange });
    await userEvent.click(screen.getByRole("button", { name: "设备" }));
    expect(onChange).toHaveBeenCalledWith("devices" satisfies TabId);
  });

  it("当前激活 tab 标记 aria-current", () => {
    renderRail({ active: "settings", onChange: () => {} });
    const settings = screen.getByRole("button", { name: "设置" });
    expect(settings).toHaveAttribute("aria-current", "true");
    const dashboard = screen.getByRole("button", { name: "仪表盘" });
    expect(dashboard).not.toHaveAttribute("aria-current");
  });
});
