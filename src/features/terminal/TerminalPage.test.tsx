/**
 * 真终端页的测试（FT1：分页与保活）。
 *
 * 用的是**真 xterm + 假传输层**：分页这条验收项问的是"切走的 tab 输出还在不在屏上"，
 * 那是 xterm 缓冲区与 DOM 的事实，桩掉终端就等于什么都没测（FT0 那版正是这么把
 * "生产路径从来没 open 过终端"遮住的——真窗口里一片黑，测试全绿）。
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import { TerminalPage } from "./TerminalPage";

import type { PtyExit } from "@/api/pty";

interface Fake {
  openArgs: Array<{ program: string; cols: number; rows: number }>;
  feeders: Map<string, (bytes: Uint8Array) => void>;
  writes: string[];
  closes: string[];
  exits: Array<(exit: PtyExit) => void>;
}

const fake: Fake = {
  openArgs: [],
  feeders: new Map(),
  writes: [],
  closes: [],
  exits: [],
};

vi.mock("@/api/pty", () => ({
  ptyApi: {
    defaultProgram: vi.fn(async () => "/bin/zsh"),
    open: vi.fn(async (args: { program: string; cols: number; rows: number; onData: (b: Uint8Array) => void }) => {
      const id = `pty-${fake.openArgs.length + 1}`;
      fake.openArgs.push({ program: args.program, cols: args.cols, rows: args.rows });
      fake.feeders.set(id, args.onData);
      return { id, pid: 4242, cols: args.cols, rows: args.rows, term: "xterm-256color" };
    }),
    write: vi.fn(async (id: string, bytes: Uint8Array) => {
      fake.writes.push(`${id}:${new TextDecoder().decode(bytes)}`);
    }),
    resize: vi.fn(async () => undefined),
    close: vi.fn(async (id: string) => {
      fake.closes.push(id);
    }),
    onExit: vi.fn(async (handler: (exit: PtyExit) => void) => {
      fake.exits.push(handler);
      return () => undefined;
    }),
  },
}));

import { ptyApi } from "@/api/pty";

const ENCODE = new TextEncoder();

/**
 * 只读**行容器**的文本：整块宿主的 textContent 会把 xterm 注入的 `<style>` 也算进去，
 * 那里面全是 CSS（`hidden`、`scrollbar` 这些词），拿它做包含断言会假绿。
 */
function rowsOf(el: HTMLElement): string {
  return Array.from(el.querySelectorAll(".xterm-rows > div"))
    .map((row) => row.textContent ?? "")
    .join("\n");
}

function hostsOf(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>("[data-testid^='terminal-host']"));
}

async function newTab(container: HTMLElement, program?: string): Promise<HTMLElement> {
  const before = hostsOf(container).length;
  if (program) {
    fireEvent.change(screen.getByTestId("terminal-program"), { target: { value: program } });
  }
  fireEvent.click(screen.getByTestId("terminal-new"));
  await waitFor(() => expect(hostsOf(container).length).toBe(before + 1));
  await waitFor(() => expect(fake.openArgs.length).toBe(before + 1));
  return hostsOf(container)[before];
}

async function renderPage() {
  const view = render(
    <I18nProvider>
      <TerminalPage />
    </I18nProvider>,
  );
  await waitFor(() => expect(screen.getByTestId("terminal-program")).toHaveValue("/bin/zsh"));
  return view;
}

beforeEach(() => {
  fake.openArgs.length = 0;
  fake.writes.length = 0;
  fake.closes.length = 0;
  fake.exits.length = 0;
  fake.feeders.clear();
  vi.mocked(ptyApi.open).mockClear();
  vi.mocked(ptyApi.close).mockClear();
});

describe("真终端页（FT1 分页）", () => {
  /**
   * FT0 漏掉的那条腿：xterm 不 `open(element)` 就一个字也不画。
   * 那时生产路径只在测试里 open 过，真窗口是一片黑——所以这条断言看的是**宿主里有没有
   * xterm 自己的节点**，不是"组件渲染没报错"。
   */
  it("起会话后终端真的挂在宿主元素里（不是一块黑的空壳）", async () => {
    const { container } = await renderPage();
    const host = await newTab(container);
    await waitFor(() => expect(host.querySelector(".xterm-screen")).toBeTruthy());
    expect(host.querySelector("textarea.xterm-helper-textarea")).toBeTruthy();
  });

  /**
   * 分页那条验收动作：开 3 个终端，第 2 个跑长任务，切到第 1 个再切回 ——
   * 第 2 个一直在跑、输出没丢、第 1 个还能打字。
   */
  it("切走再切回：另一个会话一直在跑，输出一行都没丢", async () => {
    const { container } = await renderPage();
    const tab1 = await newTab(container, "/bin/zsh");
    const tab2 = await newTab(container, "/bin/sh");
    await newTab(container, "top");
    const feed2 = fake.feeders.get("pty-2");
    expect(feed2).toBeTruthy();

    // 第 2 个"跑长任务"：一行一行喂（真终端里就是这样陆续到的字节）
    feed2?.(ENCODE.encode("LONG-1\n"));
    await waitFor(() => expect(rowsOf(tab2)).toContain("LONG-1"));

    // 切到第 1 个：第 2 个只是**不可见**，实例与缓冲区都还活着（没有任何东西被关）
    fireEvent.click(screen.getByTestId("terminal-tab-term-1"));
    await waitFor(() => expect(tab1).not.toHaveAttribute("hidden"));
    expect(tab2).toHaveAttribute("hidden");
    expect(fake.closes).toEqual([]);
    // 切走期间它还在输出：喂进去的字节继续进那一块屏
    feed2?.(ENCODE.encode("LONG-2\n"));

    // 切回来：两行都还在（"输出没丢"就是这一条），状态灯还是"在跑"
    fireEvent.click(screen.getByTestId("terminal-tab-term-2"));
    await waitFor(() => expect(tab2).not.toHaveAttribute("hidden"));
    await waitFor(() => expect(rowsOf(tab2)).toContain("LONG-1"));
    await waitFor(() => expect(rowsOf(tab2)).toContain("LONG-2"));
    expect(screen.getByTestId("terminal-dot-term-2").className).toContain("bg-emerald-500");
    expect(screen.getByTestId("terminal-status").textContent).toContain("会话在跑");

    // 第 1 个还能打字：切过去之后焦点就落在那块屏的输入元素里（键盘事件才有地方进）。
    // 注意这里只断言到"焦点归谁"——**按键字节进哪条会话**由 terminalStore 用真 xterm 钉，
    // jsdom 里合成 keydown 打不出 xterm 的 onData，硬凑只会得到一条永远绿的假断言。
    fireEvent.click(screen.getByTestId("terminal-tab-term-1"));
    await waitFor(() => expect(tab1).not.toHaveAttribute("hidden"));
    await waitFor(() => expect(tab1.contains(document.activeElement)).toBe(true));
    expect(document.activeElement?.tagName).toBe("TEXTAREA");
    expect(fake.closes).toEqual([]);
  });

  /** 关一个运行中的会话 = 杀进程：第一次点只登记"待确认"，什么都不许杀。 */
  it("关运行中的 tab 先要确认，确认后只杀它那一个", async () => {
    const { container } = await renderPage();
    await newTab(container, "/bin/zsh");
    await newTab(container, "/bin/sh");

    fireEvent.click(screen.getByTestId("terminal-close-term-1"));
    await waitFor(() => expect(screen.getByTestId("terminal-confirm")).toBeTruthy());
    expect(fake.closes).toEqual([]);
    expect(screen.getByTestId("terminal-confirm").textContent).toContain("zsh 1");

    // 取消就什么都不发生
    fireEvent.click(screen.getByTestId("terminal-confirm-no"));
    await waitFor(() => expect(screen.queryByTestId("terminal-confirm")).toBeNull());
    expect(fake.closes).toEqual([]);

    fireEvent.click(screen.getByTestId("terminal-close-term-1"));
    fireEvent.click(screen.getByTestId("terminal-confirm-yes"));
    await waitFor(() => expect(fake.closes).toEqual(["pty-1"]));
    // 另一个会话不受影响：还在表里、还在跑
    expect(hostsOf(container).length).toBe(1);
    expect(screen.getByTestId("terminal-dot-term-2").className).toContain("bg-emerald-500");
  });

  /** 已退出的会话没什么可确认，直接关（也别对着已经没了的 pty 再 close 一次）。 */
  it("进程已经退出的 tab 一点就关，不再要确认", async () => {
    const { container } = await renderPage();
    await newTab(container);
    fake.exits.forEach((handler) => handler({ id: "pty-1", exitCode: 1, signal: null } as PtyExit));
    await waitFor(() => expect(screen.getByTestId("terminal-status").textContent).toContain("已退出（1）"));

    fireEvent.click(screen.getByTestId("terminal-close-term-1"));
    await waitFor(() => expect(screen.queryByTestId("terminal-tabs")).toBeNull());
    expect(fake.closes).toEqual([]);
  });

  /** 上限的口径是"拒绝 + 说清楚"，不是悄悄把最早那个杀了。 */
  it("到上限时拒绝新开并说明原因，一个都不许被静默杀掉", async () => {
    const { container } = await renderPage();
    for (let index = 0; index < 8; index += 1) {
      await newTab(container);
    }
    expect(screen.getByTestId("terminal-new")).toBeDisabled();
    expect(screen.getByTestId("terminal-count").textContent).toContain("8/8");
    expect(fake.closes).toEqual([]);
  });

  /** 停止只杀掉进程，那块屏还要留着继续滚（与"关闭 tab"是两件事）。 */
  it("停止留下这块屏，关闭才收掉它", async () => {
    const { container } = await renderPage();
    const tab = await newTab(container);
    fake.feeders.get("pty-1")?.(ENCODE.encode("KEEP-ME\n"));
    await waitFor(() => expect(tab.textContent).toContain("KEEP-ME"));

    fireEvent.click(screen.getByTestId("terminal-stop"));
    await waitFor(() => expect(fake.closes).toEqual(["pty-1"]));
    expect(screen.getByTestId("terminal-status").textContent).toContain("会话已停止");
    expect(tab.textContent).toContain("KEEP-ME");
    expect(hostsOf(container).length).toBe(1);
  });

  /** R-08：离开页面才是批量收摊的地方，三条活会话各关一次，一条不留。 */
  it("离开页面把所有会话都关掉", async () => {
    const view = render(
      <I18nProvider>
        <TerminalPage />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-program")).toHaveValue("/bin/zsh"));
    await newTab(view.container);
    await newTab(view.container);
    await newTab(view.container);
    expect(fake.closes).toEqual([]);

    view.unmount();
    await waitFor(() => expect(fake.closes).toEqual(["pty-1", "pty-2", "pty-3"]));
  });
});
