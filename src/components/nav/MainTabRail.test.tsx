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
  it("渲染全部 13 个总 tab（默认 zh-CN 标签）", () => {
    renderRail();
    for (const tab of MAIN_TABS) {
      expect(screen.getByRole("button", { name: ZH_LABELS[tab.labelKey] })).toBeInTheDocument();
    }
    expect(MAIN_TABS).toHaveLength(13);
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

  it("未选中齿块本体让出点击、图标热区留住（透明区穿透的粒度就钉在这儿）", () => {
    // 用户报"透明部分透传没生效"的根因：整条 rail 曾被钉成实体，rail 宽 128px 而齿块只有
    // 36px，左边那一整条纯透明留白跟着一起被挡住了。现在 rail 不标实体，
    // 未选中齿块本体标 pass（只有半透明着色），图标 pad 标 solid（否则切不了页）。
    renderRail();
    const teeth = screen.getAllByRole("button");
    const inactive = teeth.filter((t) => t.getAttribute("aria-current") !== "true");
    const active = teeth.filter((t) => t.getAttribute("aria-current") === "true");
    expect(inactive.length).toBeGreaterThan(0);
    expect(active.length).toBe(1);
    for (const tooth of inactive) {
      expect(tooth.getAttribute("data-click-through")).toBe("pass");
      const pad = tooth.querySelector('[data-click-through="solid"]');
      expect(pad).toBeTruthy();
    }
    // 选中态是实心 bg-primary，本来就是实体：必须显式标 solid，
    // 否则会被 nav 的 pass 一路穿透掉，当前页的 tab 就成了点不动的空壳
    expect(active[0].getAttribute("data-click-through")).toBe("solid");
    // rail 是全项目唯一声明"可以让出点击"的区域，而且绝不能反过来钉成实体
    expect(screen.getByRole("navigation").getAttribute("data-click-through")).toBe("pass");
  });

  it("当前激活 tab 标记 aria-current", () => {
    renderRail();
    const dashboard = screen.getByRole("button", { name: "仪表盘" });
    expect(dashboard).toHaveAttribute("aria-current", "true");
    const settings = screen.getByRole("button", { name: "设置" });
    expect(settings).not.toHaveAttribute("aria-current");
  });
});
