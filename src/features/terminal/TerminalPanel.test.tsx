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
import { TerminalPanel } from "./TerminalPanel";

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

/** 环境探测的可控桩：每轮改 `env.python/env.frida` 就能演"刚 pip install 完"那种现场 */
interface EnvProbe {
  python: PythonEnv;
  frida: FridaEnv;
  fridaCalls: number;
}

/** 显式标注类型：不标的话 TS 会把 `path` 推成 `string`，于是"没配好 Python"那轮没法写 null */
const env: EnvProbe = {
  python: { configured: true, ready: true, path: "/py/3.11/bin/python3", version: "Python 3.11.9", hint: null },
  frida: { pythonReady: true, installed: true, fridaVersion: "16.5.1", fridaToolsVersion: "12.5.3", hint: null },
  fridaCalls: 0,
};

/** 保存对话框与写盘命令的桩：D090 那三条口径（取消不算失败、写完报字节数、只存过滤后的）都靠它们 */
const dialog = {
  picked: "/tmp/terminal-zsh.txt" as string | null,
  calls: 0,
  saved: [] as Array<{ path: string; content: string }>,
};

/** 设备与 adb 环境的可控桩（H-2 设备 shell 宿主要用它拿 adb 路径与在线设备列表） */
const adb = {
  env: { installed: true, path: "/sdk/platform-tools/adb" } as Record<string, unknown>,
  devices: [{ serial: "18271FDF600FL4", state: "device", transport: "usb", model: "Pixel 6" }],
};

vi.mock("@/api/device", () => ({
  deviceApi: {
    environment: vi.fn(async () => adb.env),
    list: vi.fn(async () => adb.devices),
    // 起 REPL 前置检查要问它：默认"在跑"，让每条 frida 测试都能走到起会话那一步
    fridaServerStatus: vi.fn(async () => ({ state: "running", running: true, pid: 4242 })),
  },
}));

vi.mock("@/api/dialog", () => ({
  hasNativeDialog: () => true,
  saveFile: vi.fn(async () => {
    dialog.calls += 1;
    return dialog.picked;
  }),
}));

vi.mock("@/api/hook", () => ({
  hookApi: {
    exportSave: vi.fn(async (path: string, content: string) => {
      dialog.saved.push({ path, content });
      return new TextEncoder().encode(content).length;
    }),
  },
}));

vi.mock("@/api/env", () => ({
  envApi: {
    python: vi.fn(async () => env.python),
    frida: vi.fn(async () => {
      env.fridaCalls += 1;
      return env.frida;
    }),
  },
}));

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
import type { FridaEnv, PythonEnv } from "@/api/env";

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

/** ➕ 就是新建（命令输入框按用户要求撤了：要跑别的，直接在那块屏里敲） */
async function newTab(container: HTMLElement, _program?: string): Promise<HTMLElement> {
  const before = hostsOf(container).length;
  fireEvent.click(screen.getByTestId("terminal-new"));
  await waitFor(() => expect(hostsOf(container).length).toBe(before + 1));
  await waitFor(() => expect(fake.openArgs.length).toBe(before + 1));
  return hostsOf(container)[before];
}

const fridaSettings = () => ({
  deviceSerial: "18271FDF600FL4",
  connMode: "usb" as const,
  port: "27042",
  runMode: "attach" as const,
  target: "",
});

/** 带 frida 请求渲染面板（B 区点脚本 = 传一个新 nonce 进来） */
function renderPanel(request: { scriptPath: string; settings: ReturnType<typeof fridaSettings>; nonce: number }) {
  return render(
    <I18nProvider>
      <TerminalPanel fridaLaunch={request} />
    </I18nProvider>,
  );
}

/** 设备宿主：同一个面板，只是 ➕ 起 `adb -s … shell -tt`（设备 shell 与终端/结构化同级） */
function renderDevicePanel() {
  return render(
    <I18nProvider>
      <TerminalPanel host="device" />
    </I18nProvider>,
  );
}

async function renderPage() {
  const view = render(
    <I18nProvider>
      <TerminalPanel />
    </I18nProvider>,
  );
  // 默认程序是异步问桌面要的：等到 ➕ 可用，才等于"这轮渲染真的能新建"
  await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
  return view;
}

beforeEach(() => {
  adb.env = { installed: true, path: "/sdk/platform-tools/adb" };
  adb.devices = [{ serial: "18271FDF600FL4", state: "device", transport: "usb", model: "Pixel 6" }];
  dialog.picked = "/tmp/terminal-zsh.txt";
  dialog.calls = 0;
  dialog.saved.length = 0;
  env.python = { configured: true, ready: true, path: "/py/3.11/bin/python3", version: "Python 3.11.9", hint: null };
  env.frida = { pythonReady: true, installed: true, fridaVersion: "16.5.1", fridaToolsVersion: "12.5.3", hint: null };
  env.fridaCalls = 0;
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
    // 状态不再常驻界面：断言改成"每个 tab 的悬停提示里能看到状态"
    expect(screen.getByTestId("terminal-chip-term-2").getAttribute("title")).toContain("会话在跑");

    // 第 1 个还能打字：切过去之后焦点就落在那块屏的输入元素里（键盘事件才有地方进）。
    // 注意这里只断言到"焦点归谁"——**按键字节进哪条会话**由 terminalStore 用真 xterm 钉，
    // jsdom 里合成 keydown 打不出 xterm 的 onData，硬凑只会得到一条永远绿的假断言。
    fireEvent.click(screen.getByTestId("terminal-tab-term-1"));
    await waitFor(() => expect(tab1).not.toHaveAttribute("hidden"));
    await waitFor(() => expect(tab1.contains(document.activeElement)).toBe(true));
    expect(document.activeElement?.tagName).toBe("TEXTAREA");
    expect(fake.closes).toEqual([]);
  });

  /**
   * tab 右键 → 重命名 → 回车：标题改掉，而且改的是会话表里的真值
   * （不是只改输入框的显示）。空名字必须被拒——标题清空后没人认得出这块屏连的是什么。
   */
  it("右键 tab 可以重命名，空名字不会把标题清掉", async () => {
    const { container } = await renderPage();
    await newTab(container);
    const chip = () => screen.getByTestId("terminal-chip-term-1");
    expect(chip().textContent).toContain("会话 1");

    fireEvent.contextMenu(chip());
    await waitFor(() => expect(screen.getByTestId("terminal-tab-menu")).toBeTruthy());
    fireEvent.click(screen.getByTestId("terminal-menu-rename"));
    const input = await screen.findByTestId("terminal-rename-input");

    // 先试空串：会话表应当拒绝，标题保持原样
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(screen.queryByTestId("terminal-rename-input")).toBeNull());
    expect(chip().textContent).toContain("会话 1");

    fireEvent.contextMenu(chip());
    fireEvent.click(screen.getByTestId("terminal-menu-rename"));
    const input2 = await screen.findByTestId("terminal-rename-input");
    fireEvent.change(input2, { target: { value: "签名抓取" } });
    fireEvent.keyDown(input2, { key: "Enter" });
    await waitFor(() => expect(chip().textContent).toContain("签名抓取"));
    // 悬停提示还在说状态与尺寸：撤掉常驻文字不等于把信息弄丢
    expect(chip().getAttribute("title")).toContain("会话在跑");
  });

  /** tab 默认显示「会话 N」（序号来自会话表，句子来自词典），旁边不再挂 1/8 */
  it("tab 默认显示「会话 1」「会话 2」，界面上不再有 1/8", async () => {
    const { container } = await renderPage();
    await newTab(container);
    expect(screen.getByTestId("terminal-chip-term-1").textContent).toContain("会话 1");
    expect(screen.queryByTestId("terminal-count")).toBeNull();
    await newTab(container);
    expect(screen.getByTestId("terminal-chip-term-2").textContent).toContain("会话 2");
  });

  /**
   * ✕ 一个动作做完两件事：**先停止再关闭**（用户规格，两步确认按她要求撤了）。
   * 判据是 PTY 被关**恰好一次**（不重复 close）、这块 tab 消失、别的会话不受影响——
   * 也就没有"界面没了、进程还在"的僵尸。
   */
  it("✕ 一次就停止并关闭，只关它那一个", async () => {
    const { container } = await renderPage();
    await newTab(container);
    await newTab(container);

    fireEvent.click(screen.getByTestId("terminal-close-term-1"));
    await waitFor(() => expect(fake.closes).toEqual(["pty-1"]));
    await waitFor(() => expect(screen.getAllByTestId(/^terminal-chip-/)).toHaveLength(1));
    expect(screen.getByTestId("terminal-dot-term-2").className).toContain("bg-emerald-500");
    expect(screen.queryByTestId("terminal-confirm")).toBeNull();
  });

  /** 已退出的会话没什么可确认，直接关（也别对着已经没了的 pty 再 close 一次）。 */
  it("进程已经退出的 tab 一点就关，不再要确认", async () => {
    const { container } = await renderPage();
    await newTab(container);
    fake.exits.forEach((handler) => handler({ id: "pty-1", exitCode: 1, signal: null } as PtyExit));
    await waitFor(() =>
      expect(screen.getByTestId("terminal-chip-term-1").getAttribute("title")).toContain("已退出（1）"),
    );

    fireEvent.click(screen.getByTestId("terminal-close-term-1"));
    // 关到 0 个：tab 条本身还在（➕ 得留着），但一颗 chip 都没有
    await waitFor(() => expect(screen.queryAllByTestId(/^terminal-chip-/)).toHaveLength(0));
    expect(screen.getByTestId("terminal-new")).toBeEnabled();
    expect(fake.closes).toEqual([]);
  });

  /** 上限的口径是"拒绝 + 说清楚"，不是悄悄把最早那个杀了。 */
  it("到上限时拒绝新开并说明原因，一个都不许被静默杀掉", async () => {
    const { container } = await renderPage();
    for (let index = 0; index < 8; index += 1) {
      await newTab(container);
    }
    // 到顶：➕ 自己变不可用（不靠一句提示），且**一个已有会话都不许被静默关掉**
    expect(screen.getByTestId("terminal-new")).toBeDisabled();
    expect(screen.queryByTestId("terminal-count")).toBeNull();
    expect(fake.closes).toEqual([]);
    expect(screen.getAllByTestId(/^terminal-chip-/)).toHaveLength(8);
  });

  /**
   * 路 B 的落点（第八十四轮，用户纠正之后）：**B 区点脚本 = 在这块屏上起官方 frida REPL**。
   * 判据是传给 PTY 的参数数组原样是 `-m frida_tools.repl -D <序列号> -F -l <绝对路径>`——
   * 少一个形状就不是"在跑官方 frida"，而是我们又一次自己发挥了。
   */
  it("收到 frida 会话请求就起官方 REPL，参数原样交给 PTY", async () => {
    renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(vi.mocked(ptyApi.open).mock.calls[0][0]).toMatchObject({
      program: "/py/3.11/bin/python3",
      args: ["-m", "frida_tools.repl", "-D", "18271FDF600FL4", "-F", "-l", "/hooks/hook.js"],
    });
    // tab 标题带目标与脚本：多块屏并存时她得知道哪块连的是哪个
    // 默认名就是「会话 N」；命令行（含 -l 脚本路径）在悬停里，界面不再印一遍
    expect(screen.getByTestId("terminal-chip-term-1").textContent).toContain("会话 1");
    expect(screen.getByTestId("terminal-chip-term-1").getAttribute("title")).toContain("-l /hooks/hook.js");
  });

  /**
   * 只认 nonce：**同一个请求对象被 React 重新渲染一次，绝不能再开一条 PTY**——
   * 那是真多一个进程、真多一个 attach。所以这里用"重新渲染同一份请求"来验。
   */
  it("同一个 nonce 重复渲染只起一条会话", async () => {
    const request = { scriptPath: "/hooks/a.js", settings: fridaSettings(), nonce: 7 };
    const view = renderPanel(request);
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    view.rerender(
      <I18nProvider>
        <TerminalPanel fridaLaunch={{ ...request, settings: { ...request.settings } }} />
      </I18nProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fake.openArgs.length).toBe(1);

    // nonce 变了才再起一块屏（点同一个脚本第二次也要能再起）
    view.rerender(
      <I18nProvider>
        <TerminalPanel fridaLaunch={{ ...request, nonce: 8 }} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.openArgs.length).toBe(2));
  });

  /**
   * 看门狗不许被"重渲染"清零：上一轮我就是这么写废它的
   * （界面上有多个 react-query 在轮询，任何一次 setState 都会把 setTimeout 清掉）。
   * 判据：故意在挂着卡住的会话之后再渲染一次，2 秒后仍然要报告。
   */
  it("卡住的会话即使中途重渲染，也照样报「会话没起起来」", async () => {
    vi.mocked(ptyApi.open).mockImplementationOnce((args) => {
      fake.openArgs.push({ program: args.program, cols: args.cols, rows: args.rows });
      return new Promise(() => undefined);
    });
    const view = renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    // 中途来一次重渲染（模拟 react-query 轮询回来的 setState）
    view.rerender(
      <I18nProvider>
        <TerminalPanel fridaLaunch={{ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 }} />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-error").textContent).toContain("会话没起起来"), {
      timeout: 4000,
    });
  }, 9000);

  /**
   * 黑屏不许是"零信息"：起会话那一刻就把**要跑的命令**写在屏上。
   * 第八十七轮的现场是"点完脚本一片黑、什么提示都没有、日志里连 pty_open 都没有"，
   * 那种情况下用户能做的只有猜；现在这块屏自己会说出它打算干什么。
   */
  it("屏上先写出要跑的那条命令（不是等着看黑屏）", async () => {
    const { container } = renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    await waitFor(() => expect(rowsOf(container)).toContain("> /py/3.11/bin/python3"));
    expect(rowsOf(container)).toContain("-m frida_tools.repl");
  });

  /**
   * 卡在"正在起"必须自己说出来。判据是**红色提示出现**，不是"我们等了一会儿没看到输出"——
   * 后者正是这轮之前用户面对的状态。
   */
  it("pty 一直不返回时，2 秒内报告「会话没起起来」", async () => {
    // 先照常登记这一次调用，再让 promise 永远不返回：这就是"pty 卡住不回来"的形状
    vi.mocked(ptyApi.open).mockImplementationOnce((args) => {
      fake.openArgs.push({ program: args.program, cols: args.cols, rows: args.rows });
      return new Promise(() => undefined);
    });
    renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    await waitFor(() => expect(screen.getByTestId("terminal-error").textContent).toContain("会话没起起来"), {
      timeout: 4000,
    });
  });

  /** D091 的落点：绑定包在、frida-tools 不在 ⇒ 说清缺哪个，而不是起一个报 No module 的会话。 */
  it("只装了 frida 绑定包时拒绝起会话，并说明缺的是 frida-tools", async () => {
    env.frida = { ...env.frida, fridaToolsVersion: null, installed: true };
    renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("frida-tools");
    expect(fake.openArgs).toEqual([]);
  });

  /** 探测的剪枝不能被绕过：Python 没就绪时不该去问 frida（问了也问不出东西）。 */
  it("Python 没配好时不去探 frida，直接说先去设置选解释器", async () => {
    env.python = { configured: false, ready: false, path: null, version: null, hint: null };
    renderPanel({ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(env.fridaCalls).toBe(0);
    expect(fake.openArgs).toEqual([]);
  });

  /** 没脚本不起会话：**"没加载脚本的 REPL"看着在跑、什么都不钩**，是最难查的假成功。 */
  it("脚本路径为空时不起会话", async () => {
    renderPanel({ scriptPath: "   ", settings: fridaSettings(), nonce: 1 });
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("脚本");
    expect(fake.openArgs).toEqual([]);
  });
  /**
   * 「保存日志」在真终端里也能用（C-15 的终端那一半）。
   * 判据不是"点了没报错"，而是**写出去的那段文本就是屏上那些行**、且界面报了字节数。
   */
  it("保存这块屏：写出去的是纯文本，报的是回读到的字节数", async () => {
    const { container } = await renderPage();
    const tab = await newTab(container);
    fake.feeders.get("pty-1")?.(ENCODE.encode("hello\r\n\u001b[31mRED-VALUE\u001b[0m\r\n"));
    await waitFor(() => expect(rowsOf(tab)).toContain("RED-VALUE"));

    fireEvent.click(screen.getByTestId("terminal-save"));
    await waitFor(() => expect(dialog.saved.length).toBe(1));
    const written = dialog.saved[0].content;
    expect(written).toContain("hello");
    expect(written).toContain("RED-VALUE");
    // 颜色是终端的语义，不是文件里的乱码：导出的必须是纯文本
    expect(written.includes("\u001b")).toBe(false);
    expect(screen.getByTestId("terminal-save-outcome").textContent).toContain("已存");
    expect(screen.getByTestId("terminal-save-outcome").textContent).toContain("字节");
  });

  /** 「存过滤后的那些行」：搜索框里有字，就只写匹配的行（与控制台那条口径一致）。 */
  it("搜索框里有过滤词时只存匹配的行", async () => {
    const { container } = await renderPage();
    await newTab(container);
    fake.feeders
      .get("pty-1")
      ?.(ENCODE.encode("GET /a\r\nsign=deadbeef\r\nGET /b\r\n"));
    await waitFor(() => expect(rowsOf(container)).toContain("sign=deadbeef"));
    fireEvent.change(screen.getByTestId("terminal-search"), { target: { value: "sign=" } });

    fireEvent.click(screen.getByTestId("terminal-save"));
    await waitFor(() => expect(dialog.saved.length).toBe(1));
    // 写出去的就是那一行，一个字的多余内容都没有
    expect(dialog.saved[0].content).toBe("sign=deadbeef\n");
    // 报的是"存了几行 / 屏上一共几行"（分母含工具自己写的行，所以只断言格式与分子）
    expect(screen.getByTestId("terminal-save-outcome").textContent).toMatch(/已存 1\/\d+ 行/);
  });

  /** 取消不是失败：什么都不写，也不弹一句红字。 */
  it("用户在对话框里取消：不写文件、不报失败", async () => {
    const { container } = await renderPage();
    await newTab(container);
    fake.feeders.get("pty-1")?.(ENCODE.encode("SOMETHING\r\n"));
    await waitFor(() => expect(rowsOf(container)).toContain("SOMETHING"));
    dialog.picked = null;

    fireEvent.click(screen.getByTestId("terminal-save"));
    await waitFor(() => expect(dialog.calls).toBe(1));
    expect(dialog.saved).toEqual([]);
    expect(screen.queryByTestId("terminal-save-outcome")).toBeNull();
  });

  /** 没有命中就什么都不写——连对话框都不该弹（省一次"选了路径却存出空文件"）。 */
  it("过滤词没命中时不弹对话框，直接说清什么都没写", async () => {
    const { container } = await renderPage();
    await newTab(container);
    fake.feeders.get("pty-1")?.(ENCODE.encode("GET /a\r\n"));
    await waitFor(() => expect(rowsOf(container)).toContain("GET /a"));
    fireEvent.change(screen.getByTestId("terminal-search"), { target: { value: "sign=" } });

    fireEvent.click(screen.getByTestId("terminal-save"));
    await waitFor(() => expect(screen.getByTestId("terminal-save-outcome")).toBeTruthy());
    expect(dialog.calls).toBe(0);
    expect(dialog.saved).toEqual([]);
    expect(screen.getByTestId("terminal-save-outcome").textContent).toContain("sign=");
  });

  /**
   * H-2 设备 shell（FT2）：**直接起会话**，走的是同一条 PTY 通道，只是换个程序。
   * 判据是传给 `pty_open` 的参数数组原样是 `adb -s <serial> shell -tt`——
   * 少那个 `-tt` 就没有远端 PTY，Ctrl-C/行编辑/全屏程序全都不成立（真机实测见 lib/deviceShell）。
   */
  it("设备 shell 起的是 adb -s <serial> shell -tt", async () => {
    renderDevicePanel();
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(fake.openArgs[0].program).toBe("/sdk/platform-tools/adb");
    // 参数数组原样交给 PTY：少那个 `-tt` 就没有远端 PTY，中断/行编辑/全屏程序全都不成立
    expect(vi.mocked(ptyApi.open).mock.calls[0][0].args).toEqual([
      "-s",
      "18271FDF600FL4",
      "shell",
      "-tt",
    ]);
    // 默认名仍是「会话 N」；跑的是什么放在悬停提示里（要查的时候拿得到，不占界面）
    expect(screen.getByTestId("terminal-chip-term-1").textContent).toContain("会话 1");
    expect(screen.getByTestId("terminal-chip-term-1").getAttribute("title")).toContain("/sdk/platform-tools/adb");
    expect(screen.queryByTestId("terminal-hint")).toBeNull();
  });

  /**
   * 两份菜单的分工（用户纠正过）：标签上**只有「重命名」**，
   * 复制/粘贴/复制当前路径/新建/关闭属于**屏里**的右键菜单。
   */
  it("标签右键只有重命名；屏里右键才是那五项", async () => {
    const { container } = await renderPage();
    const host = await newTab(container);

    fireEvent.contextMenu(screen.getByTestId("terminal-chip-term-1"));
    await waitFor(() => expect(screen.getByTestId("terminal-tab-menu")).toBeTruthy());
    const tabItems = screen
      .getAllByTestId(/^terminal-menu-/)
      .map((b) => (b.getAttribute("data-testid") ?? "").replace("terminal-menu-", ""));
    expect(tabItems).toEqual(["rename"]);

    // 点一下菜单外就把上一份菜单收掉（这正是"点菜单自己不关"那条修复的另一半）
    fireEvent.mouseDown(document.body);
    fireEvent.contextMenu(host);
    await waitFor(() =>
      expect(
        screen
          .getAllByTestId(/^terminal-menu-/)
          .map((b) => (b.getAttribute("data-testid") ?? "").replace("terminal-menu-", "")),
      ).toEqual(["copy", "paste", "copyPath", "new", "close"]),
    );
  });

  /** 没设备就是开不了：不许起一条 `adb -s "" shell` 之类的空会话糊弄界面。 */
  it("没有在线设备时拒绝起会话，并且一次 PTY 都不开", async () => {
    adb.devices = [{ serial: "EMULATOR36X5X9X0", state: "offline", transport: "tcp", model: "" }];
    renderDevicePanel();
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("没有在线设备");
    expect(fake.openArgs).toEqual([]);
  });

  /** adb 没配好就直说去设置里配，不去 PATH 里猜一个 adb（猜出来的和检测到的可能不是一条）。 */
  it("没配 adb 路径时报的是「去设置配 adb」", async () => {
    adb.env = { installed: false, path: undefined };
    renderDevicePanel();
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("设置");
    expect(fake.openArgs).toEqual([]);
  });

  /** 多设备时必须说清用了哪一台——静默挑第一台是会让人误判"我在这台上操作"的那种错。 */
  /**
   * 多设备时**不许静默挑一台**：这里不用界面文案证明，而是钉"用的是哪台"必须能从
   * 交给 PTY 的参数里读出来（tab 标题带着序列号，界面上不再另说一句说明文字）。
   */
  it("多台在线时用第一台，并把序列号写进 tab 标题而不是界面文案", async () => {
    adb.devices = [
      { serial: "AAA111", state: "device", transport: "usb", model: "" },
      { serial: "BBB222", state: "device", transport: "usb", model: "" },
    ];
    renderDevicePanel();
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(vi.mocked(ptyApi.open).mock.calls[0][0].args).toEqual(["-s", "AAA111", "shell", "-tt"]);
    expect(screen.getByTestId("terminal-chip-term-1").getAttribute("title")).toContain("-s AAA111");
    expect(screen.queryByTestId("terminal-hint")).toBeNull();
  });

  /**
   * 假卸载之后必须能用：StrictMode 会 mount→cleanup→mount，cleanup 里我们按 R-08 收了摊。
   * 如果动作抓着"渲染时那张表"，之后的点击全打在废表上 ⇒ 用户看到的就是"点了没反应"。
   */
  it("面板被重新挂载之后，点新建仍然起得来一条会话", async () => {
    const first = render(
      <I18nProvider>
        <TerminalPanel />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    first.unmount();

    render(
      <I18nProvider>
        <TerminalPanel />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    fireEvent.click(screen.getByTestId("terminal-new"));
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(screen.queryByTestId("terminal-error")).toBeNull();
  });

  /**
   * StrictMode（开发模式会把组件挂载两次）下也必须真的起进程。
   *
   * 加这条的原因很实际：用户的界面**只在开发模式**里跑，而我此前的测试**没有开 StrictMode**——
   * 环境和现场不一致时，"我这边全绿、你那边没反应"就没法解释。现在这一条替我把那个变量钉住了：
   * 双挂载 + 表被回收再重建之后，点脚本仍然必须调用 `pty_open`。
   */
  it("StrictMode 双挂载之后，frida 请求仍然会真的起进程", async () => {
    const { default: StrictModePkg } = await import("react");
    const view = render(
      <StrictModePkg.StrictMode>
        <I18nProvider>
          <TerminalPanel
            fridaLaunch={{ scriptPath: "/hooks/hook.js", settings: fridaSettings(), nonce: 1 }}
          />
        </I18nProvider>
      </StrictModePkg.StrictMode>,
    );
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(view.baseElement).toBeTruthy();
  });

  /** R-08：离开页面才是批量收摊的地方，三条活会话各关一次，一条不留。 */
  it("离开页面把所有会话都关掉", async () => {
    const view = render(
      <I18nProvider>
        <TerminalPanel />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("terminal-new")).toBeEnabled());
    await newTab(view.container);
    await newTab(view.container);
    await newTab(view.container);
    expect(fake.closes).toEqual([]);

    view.unmount();
    await waitFor(() => expect(fake.closes).toEqual(["pty-1", "pty-2", "pty-3"]));
  });
});
