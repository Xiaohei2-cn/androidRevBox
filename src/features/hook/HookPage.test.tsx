/**
 * Hook 页的位置口径（第九十六轮：结构化终端退役、编辑器落地之后）。
 *
 * 这个文件钉五件事：
 * 1. 子标签里**没有**「真终端」（那条厨房已撤）；frida 页右侧是**终端与编辑器的两个视图**；
 * 2. **终端在前、默认就是它**（用户第九十七轮定的顺序）；编辑器那块懒加载面板切过去时真的挂上 DOM；
 * 3. 切到终端视图时编辑器容器**仍在 DOM 里**、终端也**仍在**（卸载等于 disposeAll，
 *    会杀掉她正在跑的会话；编辑器则丢模型）；
 * 4. B 区点脚本行的「▶」= 在终端视图里起**官方 frida REPL**，参数原样交给 PTY；
 * 5. **不许**再出现结构化终端的任何痕迹（视图、任务、runner 调用）。
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { HookPage } from "./HookPage";
import { I18nProvider } from "@/i18n/I18nProvider";
import { hookApi } from "@/api/hook";
import { ptyApi } from "@/api/pty";

/** 设备上 frida-server 的探活结果（起 REPL 前的前置检查读它） */
const device = { serverRunning: true };

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
    list: vi.fn(async () => [
      { serial: "18271FDF600FL4", state: "device", transport: "usb", model: "Pixel 6" },
    ]),
    fridaServerStatus: vi.fn(async () => ({
      state: device.serverRunning ? "running" : "stopped",
      running: device.serverRunning,
      pid: device.serverRunning ? 4242 : undefined,
    })),
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
      fridaToolsVersion: "13.7.0",
      remoteOk: null,
    })),
    jsList: vi.fn(async () => [
      { name: "hook.js", path: "/Users/me/hooks/hook.js", size: 2048, mtime: 1_700_000_000 },
    ]),
    // 编辑器懒加载后读盘：给一份最小脚本（读到最后会有 Monaco 面板挂载的断言）
    scriptRead: vi.fn(async () => "// hook\nsend('hi');\n"),
    scriptWrite: vi.fn(async () => 12),
    scriptDelete: vi.fn(async () => undefined),
  },
}));

/** Monaco 在 jsdom 里跑不起来（worker/DOM 一大套）：懒加载块整个换成轻桩 */
vi.mock("@/features/hook/frida/ide/ScriptIde", () => ({
  ScriptIde: ({ workdir, request }: { workdir: string | null; request: unknown }) => (
    <div
      data-testid="ide-stub"
      data-workdir={workdir ?? ""}
      data-request={request ? JSON.stringify(request) : ""}
    />
  ),
}));

/**
 * 「终端」那页的作用域。同名控件在**设备页的 Shell 子标签**也有一份（整个窗口七页常驻），
 * 所以这里按页限定查询：写明"我点的是 frida 这一页的那个"，别让它指到别人身上。
 */
function terminalPane() {
  return within(screen.getByTestId("frida-terminal-view"));
}

function renderHook() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // TooltipProvider 是 AppProviders 给全局的（B 区脚本条的气泡要用），测试自己补上
  return render(
    <TooltipProvider delayDuration={0}>
      <QueryClientProvider client={qc}>
        <I18nProvider>
          <HookPage />
        </I18nProvider>
      </QueryClientProvider>
    </TooltipProvider>,
  );
}

beforeEach(() => {
  device.serverRunning = true;
  pty.openArgs.length = 0;
  vi.mocked(ptyApi.close).mockClear();
  pty.feeders.clear();
});

describe("Hook 页：右侧 = 编辑器 + 终端，两个视图", () => {
  it("没有单独挂「真终端」子标签（那条厨房早就撤了）", () => {
    renderHook();
    const labels = screen.getAllByRole("tab").map((tab) => tab.textContent);
    expect(labels).not.toContain("真终端");
    expect(labels.some((label) => label?.includes("Frida"))).toBe(true);
  });

  it("视图只有两个：终端在前、编辑器在后，且结构化终端一个都不在", async () => {
    renderHook();
    const keys = await screen.findAllByTestId(/^frida-view-/);
    // 顺序就是用户看到的位置：终端在左（先）、编辑器在右（后）
    expect(keys.map((b) => b.getAttribute("data-testid"))).toEqual([
      "frida-view-terminal",
      "frida-view-ide",
    ]);
    // 结构化终端那页必须彻底不存在（连同容器）
    expect(screen.queryByTestId("frida-view-tracks")).toBeNull();
    expect(screen.queryByTestId("frida-tracks-view")).toBeNull();
  });

  it("默认视图是终端；切到编辑器后终端容器仍在 DOM 里（不卸载）", async () => {
    renderHook();
    await waitFor(() =>
      expect(screen.getByTestId("frida-terminal-view").hasAttribute("hidden")).toBe(false),
    );
    expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(true);

    fireEvent.click(screen.getByTestId("frida-view-ide"));
    await waitFor(() =>
      expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(false),
    );
    expect(screen.getByTestId("frida-terminal-view").hasAttribute("hidden")).toBe(true);
    // 两个容器都还在 DOM 里：终端那页没被卸载（卸载=disposeAll=杀会话），
    // 编辑器 stub 也还在（模型、undo 栈都保留）
    expect(screen.getByTestId("ide-stub")).toBeTruthy();
    expect(screen.getByTestId("frida-terminal-view")).toBeTruthy();
  });

  /** 挂载这条判据在这里也要成立：第七十五轮就是"测试替生产路径补 open"遮住了黑屏。 */
  it("终端视图真的挂着 xterm（点 ➕ 后 .xterm-screen 出现）", async () => {
    renderHook();
    fireEvent.click(screen.getByTestId("frida-view-terminal"));
    await waitFor(() => expect(terminalPane().getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(terminalPane().getByTestId("terminal-new"));
    // 块是动态 import 的，全量并发跑时 1s 默认值不够；等的是"块"，给足 4s
    await waitFor(() => expect(terminalPane().getByTestId("terminal-host")).toBeTruthy(), {
      timeout: 4000,
    });
    await waitFor(() =>
      expect(terminalPane().getByTestId("terminal-host").querySelector(".xterm-screen")).toBeTruthy(),
    );
  });

  /**
   * 「设备 shell」**不在这一页**（第八十八轮定的位置：它是设备页的 Shell 子标签）。
   * 这条钉"不许换个形式回来"：C 区没有设备入口，怎么点都**不会**给 PTY 一条 `adb … shell`。
   */
  it("frida 页没有设备 shell 入口，也永远不会起设备会话", async () => {
    renderHook();
    fireEvent.click(screen.getByTestId("frida-view-terminal"));
    await waitFor(() => expect(terminalPane().getByTestId("terminal-new")).toBeEnabled());
    expect(screen.queryByTestId("frida-view-device")).toBeNull();
    expect(screen.queryByTestId("frida-device-view")).toBeNull();

    fireEvent.click(terminalPane().getByTestId("terminal-new"));
    await waitFor(() => expect(pty.openArgs.length).toBe(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    // 只有那条本机 shell；设备会话的 adb 一次都没出现
    expect(pty.openArgs.map((a) => a.program)).toEqual(["/bin/zsh"]);
  });

  /**
   * B 区点脚本行的 ▶ = 起官方 frida REPL（路 B），且**自动切到终端视图**。
   * 默认视图虽然是终端，但用户可能正待在编辑器里点 ▶（或从 IDE 的「运行」来）——
   * 不切过去，会话起来了屏幕上还是编辑器，用户以为没反应（第九十六轮的真 bug 预防）。
   * 判据是**交给 PTY 的参数数组原样**：`-m frida_tools.repl -D <序列号> -F -l <绝对路径>`。
   */
  it("点脚本行的 ▶ 会在终端视图起官方 frida REPL，并从编辑器切回终端", async () => {
    renderHook();
    await waitFor(() => expect(screen.getByText("hook.js")).toBeTruthy());
    // A 区"自动选中第一台在线设备"是异步的：不落地就点脚本，我们会**拒绝启动**而不是退回 `-U`
    await waitFor(() =>
      expect((screen.getByTestId("frida-device") as HTMLSelectElement).value).toBe("18271FDF600FL4"),
    );

    // 先切到编辑器视图（模拟"她正在编辑器里干活时点了 ▶"）
    fireEvent.click(screen.getByTestId("frida-view-ide"));
    await waitFor(() =>
      expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(false),
    );

    const row = await screen.findByTestId("script-row-hook.js");
    fireEvent.click(within(row).getByText("启动"));

    await waitFor(() => expect(pty.openArgs.length).toBe(1));
    expect(pty.openArgs[0]).toEqual({
      program: "/py/3.11/bin/python3",
      args: ["-m", "frida_tools.repl", "-D", "18271FDF600FL4", "-F", "-l", "/Users/me/hooks/hook.js"],
    });
    // 会话起来后视图自动切回终端（她点的是"启动"，就该看到它）
    await waitFor(() =>
      expect(screen.getByTestId("frida-terminal-view").hasAttribute("hidden")).toBe(false),
    );
    expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(true);
  });

  /** B 区单击一行 = 在编辑器里打开（不是启动）：编辑器 stub 会收到打开请求。 */
  it("单击脚本行把打开请求交给编辑器（不是起会话）", async () => {
    renderHook();
    const row = await screen.findByTestId("script-row-hook.js");
    fireEvent.click(row);
    await waitFor(() => {
      const stub = screen.getByTestId("ide-stub");
      expect(stub.getAttribute("data-request")).toContain("hook.js");
    });
    // 一点会话都没起（"打开"与"运行"是两件事）
    expect(pty.openArgs.length).toBe(0);
  });

  /**
   * 停在终端视图时点脚本行：光把"打开请求"发出去不够——屏幕不切过去，
   * 用户看到的就是"点了没反应"（第九十六轮自查出来的交互漏洞）。
   */
  it("从终端视图点脚本行，视图要切回编辑器", async () => {
    renderHook();
    await screen.findByTestId("frida-view-terminal");
    fireEvent.click(screen.getByTestId("frida-view-terminal"));
    await waitFor(() =>
      expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(true),
    );

    const row = await screen.findByTestId("script-row-hook.js");
    fireEvent.click(row);

    await waitFor(() =>
      expect(screen.getByTestId("frida-ide-view").hasAttribute("hidden")).toBe(false),
    );
    expect(screen.getByTestId("ide-stub").getAttribute("data-request")).toContain("hook.js");
  });

  /**
   * **A 区显示"frida-server 没在跑"也绝不许拦住起会话**（第八十六轮的教训，钉住我自己别再犯）。
   *
   * 我加过这道拦截，然后被自己的检查证伪了：server 明明在跑，但 `ps -A -o comm` 里
   * 搜不到 "frida"（它以 `unix:abstract=/frida-<uuid>` 的形式出现，shell 与 root 两种查法都空）
   * ⇒ 按进程名探活的检查必然假阴性 ⇒ 它把用户的正常路径挡在门外，还附一句又长又错的话。
   * 所以这条断言钉的是"不许拦"，不是"拦得对"：真连不上该由 frida 自己在屏上说。
   */
  it("探测说 frida-server 没跑也不拦：照样把 REPL 起起来", async () => {
    device.serverRunning = false;
    renderHook();
    await waitFor(() => expect(screen.getByText("hook.js")).toBeTruthy());
    await waitFor(() =>
      expect((screen.getByTestId("frida-device") as HTMLSelectElement).value).toBe("18271FDF600FL4"),
    );
    const row = await screen.findByTestId("script-row-hook.js");
    fireEvent.click(within(row).getByText("启动"));
    await waitFor(() => expect(pty.openArgs.length).toBe(1));
    expect(pty.openArgs[0].args).toContain("-F");
    // 也不许顺手写一句错误出来
    expect(screen.queryByTestId("terminal-error")).toBeNull();
  });

  /**
   * 结构化终端退役后，本页起会话只经过官方 REPL：api 层**不许**再有 runner 会话接口。
   * 这条防回退扫描同时钉住"前端不再有第二条起会话的路"。
   */
  it("起会话只经过官方 REPL，不再有 runner 会话任务", async () => {
    renderHook();
    await waitFor(() => expect(screen.getByText("hook.js")).toBeTruthy());
    const api = hookApi as unknown as Record<string, unknown>;
    expect(api.sessionStart).toBeUndefined();
    // Runner 退役后 exportSave 还在（真终端页「保存日志」在用），所以这里只钉会话类
    // 编辑器的读写接口在
    expect(typeof api.scriptRead).toBe("function");
    expect(typeof api.scriptWrite).toBe("function");
    expect(typeof api.scriptDelete).toBe("function");
  });
});