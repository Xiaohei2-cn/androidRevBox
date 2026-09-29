/**
 * Hook 页的位置口径（第八十四轮，用户纠正之后重写）。
 *
 * 之前这里测的是"真终端子标签懒加载"——那个子标签本身是**做错的东西**：
 * 她要的是把 frida 右侧那一整块控制台换成真终端，不是再盖一间厨房。
 * 所以这个文件现在钉四件事：
 * 1. 子标签里**没有**「真终端」了（防止那个位置以另一种形式回来）；
 * 2. frida 页右侧默认就是真终端，而且终端**真的挂上了 DOM**（第七十五轮那条黑屏缺陷的判据在这里也生效）；
 * 3. 切到"轨道"视图时终端容器**仍在 DOM 里**——卸载等于 disposeAll，会杀掉她的会话；
 * 4. B 区点脚本 = 在这块屏上起**官方 frida REPL**，参数原样交给 PTY。
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { HookPage } from "./HookPage";
import { I18nProvider } from "@/i18n/I18nProvider";
import { hookApi } from "@/api/hook";

const pty = {
  openArgs: [] as Array<{ program: string; args: string[] }>,
  feeders: new Map<string, (bytes: Uint8Array) => void>(),
};

vi.mock("@/api/pty", () => ({
  ptyApi: {
    defaultProgram: vi.fn(async () => "/bin/zsh"),
    open: vi.fn(
      async (args: {
        program: string;
        args: string[];
        cols: number;
        rows: number;
        onData: (b: Uint8Array) => void;
      }) => {
      const id = `pty-${pty.openArgs.length + 1}`;
      pty.openArgs.push({ program: args.program, args: args.args });
      pty.feeders.set(id, args.onData);
      return { id, pid: 4242, cols: args.cols, rows: args.rows, term: "xterm-256color" };
      },
    ),
    write: vi.fn(async () => undefined),
    resize: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
    onExit: vi.fn(async () => () => undefined),
  },
}));

vi.mock("@/api/env", () => ({
  envApi: {
    python: vi.fn(async () => ({
      configured: true,
      ready: true,
      path: "/py/3.11/bin/python3",
      version: "Python 3.11.9",
      hint: null,
    })),
    frida: vi.fn(async () => ({
      pythonReady: true,
      installed: true,
      fridaVersion: "17.17.0",
      fridaToolsVersion: "13.7.0",
      hint: null,
    })),
  },
}));

// B 区的脚本列表由"工作目录配好了吗"门控（config 键），不 mock 就永远是空表
vi.mock("@/api/config", () => ({
  configApi: {
    get: vi.fn(async () => "/Users/me/hooks"),
    set: vi.fn(async () => undefined),
    all: vi.fn(async () => ({})),
  },
}));

vi.mock("@/api/device", () => ({
  deviceApi: {
    environment: vi.fn(async () => ({ installed: true, path: "/sdk/adb" })),
    list: vi.fn(async () => [{ serial: "18271FDF600FL4", state: "device", transport: "usb", model: "Pixel 6" }]),
    fridaServerStatus: vi.fn(async () => ({ state: "stopped" })),
  },
}));

vi.mock("@/api/hook", () => ({
  hookApi: {
    preflight: vi.fn(async () => ({
      adbOk: true,
      pythonOk: true,
      pythonPath: "/py/3.11/bin/python3",
      fridaOk: true,
      fridaVersion: "17.17.0",
      runnerOk: true,
      remoteOk: null,
    })),
    jsList: vi.fn(async () => [
      { name: "hook.js", path: "/Users/me/hooks/hook.js", size: 2048, mtime: 1_700_000_000 },
    ]),
    sessionStart: vi.fn(async () => "task-1"),
  },
}));

vi.mock("@/api/task", () => ({
  taskApi: {
    cancel: vi.fn(async () => undefined),
    output: vi.fn(async () => ({ lines: [] })),
    status: vi.fn(async () => ({ state: "running" })),
    subscribe: vi.fn(async () => () => undefined),
  },
}));

function renderHook() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>
        <HookPage />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  pty.openArgs.length = 0;
  pty.feeders.clear();
  vi.mocked(hookApi.sessionStart).mockClear();
});

describe("Hook 页：frida 右侧那块就是真终端", () => {
  it("没有单独挂「真终端」子标签（那条厨房已经撤了）", () => {
    renderHook();
    const labels = screen.getAllByRole("tab").map((tab) => tab.textContent);
    expect(labels).not.toContain("真终端");
    expect(labels.some((label) => label?.includes("Frida"))).toBe(true);
  });

  /** 挂载这条判据在这里也要成立：第七十五轮就是"测试替生产路径补 open"遮住了黑屏。 */
  it("右侧默认是真终端视图，并且终端真的挂在 DOM 里", async () => {
    renderHook();
    await waitFor(() => expect(screen.getByTestId("frida-terminal-view")).toBeTruthy());
    expect(screen.getByTestId("frida-terminal-view").hasAttribute("hidden")).toBe(false);
    // 一块屏要真正"挂上"才有 xterm 自己的节点（起会话前这里连元素都没有，那是正常的空面板）
    // 等的是**懒加载的那块面板本身**：不等到它就点，测的是"点了个不存在的按钮"
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeTruthy());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(screen.getByTestId("terminal-host")).toBeTruthy());
    await waitFor(() =>
      expect(screen.getByTestId("terminal-host").querySelector(".xterm-screen")).toBeTruthy(),
    );
  });

  /** 切视图只改 hidden：**卸载 = disposeAll = 杀掉她正在跑的会话**，那是不可接受的。 */
  it("切到轨道视图时终端容器还在 DOM 里（不卸载、不杀会话）", async () => {
    renderHook();
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeTruthy());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(screen.getByTestId("terminal-tabs")).toBeTruthy());

    fireEvent.click(screen.getByTestId("frida-view-tracks"));
    await waitFor(() => expect(screen.getByTestId("frida-tracks-view").hasAttribute("hidden")).toBe(false));
    expect(screen.getByTestId("frida-terminal-view").hasAttribute("hidden")).toBe(true);
    // 那一块屏没有被从 DOM 里拿掉：会话表与 xterm 实例都还活着
    expect(screen.getByTestId("terminal-tabs")).toBeTruthy();
  });

  /**
   * B 区点脚本 = 起官方 frida REPL（路 B）。
   * 判据是**交给 PTY 的参数数组原样**：`-m frida_tools.repl -D <序列号> -F -l <绝对路径>`。
   * 少一个形状就不是"在跑官方 frida"，而是我们又一次自己发挥了。
   */
  it("双击脚本会在右侧起官方 frida REPL，并且不走我们那条 runner", async () => {
    renderHook();
    await waitFor(() => expect(screen.getByText("hook.js")).toBeTruthy());
    // A 区"自动选中第一台在线设备"是异步的：不落地就点脚本，我们会**拒绝启动**而不是退回 `-U`
    await waitFor(() =>
      expect((screen.getByTestId("frida-device") as HTMLSelectElement).value).toBe("18271FDF600FL4"),
    );
    fireEvent.doubleClick(screen.getByText("hook.js"));

    await waitFor(() => expect(pty.openArgs.length).toBe(1));
    expect(pty.openArgs[0]).toEqual({
      program: "/py/3.11/bin/python3",
      args: ["-m", "frida_tools.repl", "-D", "18271FDF600FL4", "-F", "-l", "/Users/me/hooks/hook.js"],
    });
    // 默认视图下不该再去起我们自己的 runner 任务（两条路各管各的会话类型）
    expect(vi.mocked(hookApi.sessionStart)).not.toHaveBeenCalled();
  });
});
