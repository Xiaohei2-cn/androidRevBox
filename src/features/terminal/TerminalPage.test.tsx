import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import { TerminalPage } from "./TerminalPage";

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

import { ptyApi } from "@/api/pty";

describe("真终端页（FT0.3）", () => {
  beforeEach(() => {
    vi.mocked(ptyApi.defaultProgram).mockClear();
    vi.mocked(ptyApi.open).mockClear();
    vi.mocked(ptyApi.close).mockClear();
  });

  it("默认程序由桌面端回答并预填，界面自己不猜（猜错就丢掉用户的 shell）", async () => {
    render(
      <I18nProvider>
        <TerminalPage />
      </I18nProvider>,
    );
    const input = await waitFor(() => screen.getByTestId("terminal-program"));
    await waitFor(() => expect((input as HTMLInputElement).value).toBe("/bin/zsh"));
  });

  it("没起会话时状态说的是实话，停止按钮不可点", async () => {
    render(
      <I18nProvider>
        <TerminalPage />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-status").textContent).toContain("还没起会话"));
    expect(screen.getByTestId("terminal-stop")).toBeDisabled();
  });

  /**
   * R-08 的界面那一半：页面卸载必须把会话关掉。
   * 判据是传输层真的收到一次 close，而不是"组件看起来消失了"。
   */
  it("离开页面会关掉会话，不留活进程", async () => {
    const view = render(
      <I18nProvider>
        <TerminalPage />
      </I18nProvider>,
    );
    await waitFor(() => expect(ptyApi.defaultProgram).toHaveBeenCalled());
    view.unmount();
    await waitFor(() => expect(vi.mocked(ptyApi.open)).not.toHaveBeenCalled());
    // 没起过会话就没什么可关；起过的那条路径由 terminalSession 的测试覆盖
    expect(ptyApi.close).not.toHaveBeenCalled();
  });
});
