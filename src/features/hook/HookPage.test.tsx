/**
 * Hook 主标签的懒加载口径（第八十三轮）。
 *
 * 为什么要单独一条测试：第七十五轮 xterm 被直接 import 进主块，启动关键路径上多了一整块
 * "只有点开才用得到的东西"（1,259 → 1,271 kB）。改成懒加载之后，"默认真的没加载"必须是
 * **可断言的事实**，不然下次有人"顺手"在 HookPage 里 import 一下，分块就悄悄没了——
 * 那种退化不会让任何现有测试变红。
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import { HookPage } from "./HookPage";

// 真终端页要 Tauri 的 pty 通道；这里只关心"它有没有被载入"，把通道桩掉即可
vi.mock("@/api/pty", () => ({
  ptyApi: {
    defaultProgram: vi.fn(async () => "/bin/zsh"),
    open: vi.fn(async () => ({ id: "pty-1", pid: 1, cols: 80, rows: 24, term: "xterm-256color" })),
    write: vi.fn(async () => undefined),
    resize: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
    onExit: vi.fn(async () => () => undefined),
  },
}));

function renderHook() {
  // frida 子标签里有用 react-query 的探测，包一层 Provider（retry:false：
  // 测不动的查询在这里失败是预期，让它退避重试只会把测试拖成 5 秒）
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>
        <HookPage />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

/**
 * Radix 的 TabsTrigger 是在 **`onMouseDown`** 里切 tab 的（v1.1.21 的实现），
 * `fireEvent.click` 只派发 click，于是点了不切——这条 helper 存在的全部理由就是
 * 别让下一个人再把"测试没生效"读成"功能没生效"。
 */
function selectTab(name: string) {
  const tab = screen.getByRole("tab", { name });
  fireEvent.mouseDown(tab);
  fireEvent.click(tab);
}

describe("Hook 主标签（真终端懒加载）", () => {
  it("默认停在 frida 子标签，真终端没有被载入", () => {
    renderHook();
    // 子标签本身在（那是个 tab 触发器），但它的内容——终端页的输入框——不该存在
    expect(screen.getByRole("tab", { name: "真终端" })).toBeTruthy();
    expect(screen.getByTestId("terminal-not-loaded")).toBeTruthy();
    expect(screen.queryByTestId("terminal-program")).toBeNull();
    expect(screen.queryByTestId("terminal-new")).toBeNull();
  });

  /** 点一次就该把 chunk 拉进来；拉进来之后**不许再卸载**（会话保活的根）。 */
  it("点开真终端就把页面载入，切走也不卸载", async () => {
    renderHook();
    selectTab("真终端");
    const input = await screen.findByTestId("terminal-program");
    expect(input).toBeTruthy();
    expect(screen.queryByTestId("terminal-not-loaded")).toBeNull();

    selectTab("Frida");
    await waitFor(() => expect(screen.getByTestId("terminal-program")).toBeTruthy());
  });
});
