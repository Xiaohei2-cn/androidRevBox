/**
 * 托管页那块交互终端的三条判据：
 * 1. 起进程的参数里必须是 `./文件名`——Android 的 shell 按 PATH 找裸文件名，
 *    真机上就报过 `id_android_svr64: inaccessible or not found`；
 * 2. 同一个 nonce 只开一块屏（父组件重渲染不该多开进程）；
 * 3. 面板卸载要把会话全收掉（页面都没了还留一个跑着的调试服务端是最难查的泄漏）。
 */
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";
import { ptyApi } from "@/api/pty";
import { HostedTerminal, type HostedTerminalHandle, type HostedTerminalRequest } from "./HostedTerminal";

const opened: Array<{ program: string; args: string[] }> = [];

vi.mock("@/api/pty", () => ({
  ptyApi: {
    defaultProgram: vi.fn(async () => "/bin/zsh"),
    open: vi.fn(async (args: { program: string; args: string[] }) => {
      opened.push({ program: args.program, args: args.args });
      return { id: `pty-${opened.length}`, pid: 4242, cols: 80, rows: 24, term: "xterm-256color" };
    }),
    write: vi.fn(async () => undefined),
    resize: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
    onExit: vi.fn(async () => () => undefined),
  },
}));

function renderTerminal(props: {
  adbPath?: string | null;
  /** 必须区分"没选中设备(null)"和"测试没写(undefined)"：写成 ?? 会把 null 又填回默认值 */
  serial?: string | null | undefined;
  request: HostedTerminalRequest | null;
  /** 父组件拿的那个句柄：收屏（`closeFor`）要从这里进来 */
  ref?: { current: HostedTerminalHandle | null };
}) {
  return render(
    <I18nProvider>
      <HostedTerminal
        ref={props.ref}
        adbPath={props.adbPath ?? "/sdk/adb"}
        serial={props.serial === undefined ? "PIXEL-1" : props.serial}
        request={props.request}
      />
    </I18nProvider>,
  );
}

const REQ = { dir: "/data/local/tmp", name: "id_android_svr64", args: ["-p", "2345"], nonce: 1 };
const REQ2 = { dir: "/data/local/tmp", name: "helper", args: [], nonce: 2 };

beforeEach(() => {
  opened.length = 0;
  vi.mocked(ptyApi.close).mockClear();
});

describe("托管页交互终端", () => {
  it("远端命令带 ./（不带就会 inaccessible or not found）", async () => {
    renderTerminal({ request: REQ });
    await waitFor(() => expect(opened.length).toBe(1));
    expect(opened[0].program).toBe("/sdk/adb");
    expect(opened[0].args).toEqual([
      "-s",
      "PIXEL-1",
      "shell",
      "-tt",
      `cd '/data/local/tmp' && './id_android_svr64' '-p' '2345'`,
    ]);
  });

  it("同一个 nonce 只开一块屏：重渲染不多开进程", async () => {
    const view = renderTerminal({ request: REQ });
    await waitFor(() => expect(opened.length).toBe(1));
    view.rerender(
      <I18nProvider>
        <HostedTerminal adbPath="/sdk/adb" serial="PIXEL-1" request={REQ} />
      </I18nProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(opened.length).toBe(1);
  });

  it("没选中设备就不开屏", async () => {
    renderTerminal({ serial: null, request: REQ });
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(opened.length).toBe(0);
    expect(screen.getByText("没有在线设备")).toBeInTheDocument();
  });

  it("面板卸载即收会话：不留跑着的调试服务端", async () => {
    const view = renderTerminal({ request: REQ });
    await waitFor(() => expect(opened.length).toBe(1));
    view.unmount();
    await waitFor(() => expect(vi.mocked(ptyApi.close)).toHaveBeenCalled());
  });

  it("closeFor 只收「这台设备 + 这个文件」的屏，并报告收了几块", async () => {
    // 「终止」之后父组件要能把那块屏一起收掉：留着跑完的屏看着就是没停干净。
    // 判据两头都要钉：该收的收了（屏上标签没了、pty 被关），不该收的一块都不许动。
    const handle: { current: HostedTerminalHandle | null } = { current: null };
    const view = renderTerminal({ request: REQ, ref: handle });
    await waitFor(() => expect(opened.length).toBe(1));
    view.rerender(
      <I18nProvider>
        <HostedTerminal adbPath="/sdk/adb" serial="PIXEL-1" request={REQ2} ref={handle} />
      </I18nProvider>,
    );
    await waitFor(() => expect(opened.length).toBe(2));
    expect(screen.getByText("id_android_svr64")).toBeTruthy();
    expect(screen.getByText("helper")).toBeTruthy();

    expect(handle.current?.closeFor("PIXEL-1", "id_android_svr64")).toBe(1);
    await waitFor(() => expect(screen.queryByText("id_android_svr64")).toBeNull());
    expect(screen.getByText("helper")).toBeTruthy();
    expect(vi.mocked(ptyApi.close)).toHaveBeenCalledTimes(1);

    // 换过设备之后那条 adb 连接上的屏不算当前设备的：0 块，也一口都不关
    expect(handle.current?.closeFor("OTHER-9", "helper")).toBe(0);
    expect(vi.mocked(ptyApi.close)).toHaveBeenCalledTimes(1);
    expect(handle.current?.closeFor("PIXEL-1", "helper")).toBe(1);
    await waitFor(() => expect(screen.queryByText("helper")).toBeNull());
    await waitFor(() => expect(vi.mocked(ptyApi.close)).toHaveBeenCalledTimes(2));
    // 屏都收光了再点停止：还是 0，不能凭空许诺"屏也关了"
    expect(handle.current?.closeFor("PIXEL-1", "helper")).toBe(0);
  });
});
