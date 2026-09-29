/**
 * B 区脚本条的显示口径（用户反馈：名字才是主角）。
 *
 * 上一版把 名字 / 大小 / 修改时间 挤在同一行：右边那两列吃掉 130+ px，
 * 名字被 truncate 成「hook_wechat_dexdu…」、只有 hover 才看得到全名——
 * 而她选脚本靠的就是名字，认不出名字等于这一条失败。
 *
 * 现在钉四件事：
 * 1. 行内只有名字，大小与时间不出现在行里；
 * 2. 名字不再靠原生 title 藏起来（那是"要 hover 才看得见"的老毛病）；
 * 3. 大小 / 修改时间 / 完整路径的唯一去处是气泡内容（ScriptMeta 直接渲染来验）；
 * 4. 那条仍然是可双击启动的 tooltip 触发器。
 *
 * ⚠️ "鼠标移上去气泡真的弹出来"这条不在这里测：jsdom 没有排版，Radix 浮层
 * 一挂载就要 ~14 s（@floating-ui 在 jsdom 里把 getComputedStyle 当循环用）。
 * 一条要跑 14 秒、且测的是浮层库而不是我们的判据的测试没有价值——
 * 气泡的弹出归真机手工矩阵（docs/frida_terminal_phases.md §5）。
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { I18nProvider } from "@/i18n/I18nProvider";
import type { JsFileDto } from "@/api/hook";
import { ScriptList, ScriptMeta, formatMtime, formatSize } from "./ScriptList";

const { SCRIPT } = vi.hoisted(() => ({
  SCRIPT: {
    name: "hook_wechat_dexdump_and_rpc_bridge_v3.js",
    path: "/Users/me/hooks/hook_wechat_dexdump_and_rpc_bridge_v3.js",
    size: 2048,
    mtime: 1_700_000_000,
  } as JsFileDto,
}));

vi.mock("@/api/config", () => ({
  configApi: {
    get: vi.fn(async () => "/Users/me/hooks"),
    set: vi.fn(async () => undefined),
  },
}));

vi.mock("@/api/hook", () => ({
  hookApi: { jsList: vi.fn(async () => [SCRIPT]) },
}));

// 选目录用原生对话框；测试里既不弹也不该弹
vi.mock("@/api/dialog", () => ({
  pickDirectory: vi.fn(async () => null),
}));

function withinProviders(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <TooltipProvider delayDuration={0}>
      <QueryClientProvider client={qc}>
        <I18nProvider>{ui}</I18nProvider>
      </QueryClientProvider>
    </TooltipProvider>,
  );
}

describe("ScriptList：名字在行里，大小与时间在气泡里", () => {
  it("行内只写名字：长名字整行归名字，不再挤大小与时间", async () => {
    withinProviders(<ScriptList running={false} onStart={vi.fn()} />);
    const name = await screen.findByText(SCRIPT.name);
    const row = screen.getByTestId(`script-row-${SCRIPT.name}`);

    // 这一条的全部文字就是名字（外加 hover 才现身的「启动」按钮）
    expect(row.textContent).toBe(`${SCRIPT.name}启动`);
    expect(row).not.toHaveTextContent(formatSize(SCRIPT.size));
    expect(row).not.toHaveTextContent(formatMtime(SCRIPT.mtime));
    // 名字占满整行：既不再 truncate，也不靠 title 藏全名
    expect(name).not.toHaveAttribute("title");
    expect(name.className).toContain("line-clamp-2");
    expect(name.className).not.toContain("truncate");
  });

  it("那条就是气泡的触发器（Radix 在 trigger 上打 data-state）", async () => {
    withinProviders(<ScriptList running={false} onStart={vi.fn()} />);
    await screen.findByText(SCRIPT.name);
    const row = screen.getByTestId(`script-row-${SCRIPT.name}`);
    expect(row).toHaveAttribute("data-state", "closed");
    // 弹出本身（closed → delayed-open）见文件头：jsdom 下测它等于测浮层库
  });

  it("气泡内容收着完整名字、路径、大小、修改时间与双击语义", () => {
    withinProviders(<ScriptMeta file={SCRIPT} />);
    const bubble = screen.getByText(SCRIPT.name).closest("div");
    expect(bubble).toHaveTextContent(SCRIPT.path);
    expect(bubble).toHaveTextContent(formatSize(SCRIPT.size)); // 2.0 KB
    expect(bubble).toHaveTextContent(formatMtime(SCRIPT.mtime)); // 2023-11-15 06:13（随本机时区）
    expect(bubble).toHaveTextContent("双击以当前设置启动会话");
  });

  it("气泡不吃交互：双击那条仍然启动会话", async () => {
    const onStart = vi.fn();
    withinProviders(<ScriptList running={false} onStart={onStart} />);
    await screen.findByText(SCRIPT.name);
    fireEvent.doubleClick(screen.getByTestId(`script-row-${SCRIPT.name}`));
    await waitFor(() => expect(onStart).toHaveBeenCalledTimes(1));
    expect(onStart).toHaveBeenCalledWith(SCRIPT);
  });
});
