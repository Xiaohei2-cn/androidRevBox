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

  it("穿透只给齿块左边的留白与未选中齿块本体：tab 之间的小空隙必须接住", () => {
    // 用户第二轮收窄要求：第一版整条栏标 pass，把齿块之间那 6px 缝（gap-1.5）也一起送了出去。
    // 现在留白是**按行**声明的（滚动、选中态变宽都不会错位），而缝属于行与行之间的 nav，
    // nav 自己没有标记 → 默认接住。
    renderRail();
    const nav = screen.getByRole("navigation");
    expect(nav.getAttribute("data-click-through")).toBeNull();

    const teeth = screen.getAllByRole("button");
    const inactive = teeth.filter((t) => t.getAttribute("aria-current") !== "true");
    const active = teeth.filter((t) => t.getAttribute("aria-current") === "true");
    expect(active).toHaveLength(1);
    for (const tooth of inactive) {
      expect(tooth.getAttribute("data-click-through")).toBe("pass"); // 半透明本体让出
      expect(tooth.querySelector('[data-click-through="solid"]')).toBeTruthy(); // 图标热区接住
    }
    // 选中态是实心 bg-primary + 中文标签，本来就是实体，必须显式 solid
    expect(active[0].getAttribute("data-click-through")).toBe("solid");

    // 每一行只有一个留白带 + 顶部带，且它们都不是按钮；行容器本身不许带标记
    const rows = nav.querySelectorAll(":scope > div");
    expect(rows).toHaveLength(13);
    for (const row of rows) {
      expect(row.getAttribute("data-click-through")).toBeNull();
      // 行里"让出点击"的非按钮元素只有一个：齿块左边那条留白带
      // （未选中齿块本体也标了 pass，但它是 button，另一段断言在管）
      expect(row.querySelectorAll('span[data-click-through="pass"]')).toHaveLength(1);
      expect(row.querySelector("button")).toBeTruthy();
    }
    // 顶部那 56px 是 nav 自己的 padding → 不标任何穿透声明（接住点击）
    expect(nav.querySelectorAll(":scope > span[data-click-through=\"pass\"]")).toHaveLength(0);
    expect(nav.className).toContain("pt-14");
  });

  it("当前激活 tab 标记 aria-current", () => {
    renderRail();
    const dashboard = screen.getByRole("button", { name: "仪表盘" });
    expect(dashboard).toHaveAttribute("aria-current", "true");
    const settings = screen.getByRole("button", { name: "设置" });
    expect(settings).not.toHaveAttribute("aria-current");
  });
});
