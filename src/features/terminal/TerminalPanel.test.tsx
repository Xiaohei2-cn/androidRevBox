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

async function renderPage() {
  const view = render(
    <I18nProvider>
      <TerminalPanel />
    </I18nProvider>,
  );
  await waitFor(() => expect(screen.getByTestId("terminal-program")).toHaveValue("/bin/zsh"));
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
    expect(screen.getByTestId("terminal-tabs").textContent).toContain("attach 自动前台 · hook.js");
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
    // 报的是"存了几行 / 屏上一共几行"，让她知道自己存的确实是被筛过的那部分
    expect(screen.getByTestId("terminal-save-outcome").textContent).toContain("1/3");
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
    await renderPage();
    fireEvent.click(screen.getByTestId("terminal-device-shell"));
    await waitFor(() => expect(fake.openArgs.length).toBe(1));
    expect(fake.openArgs[0].program).toBe("/sdk/platform-tools/adb");
    // 参数数组原样交给 PTY：少那个 `-tt` 就没有远端 PTY，中断/行编辑/全屏程序全都不成立
    expect(vi.mocked(ptyApi.open).mock.calls[0][0].args).toEqual([
      "-s",
      "18271FDF600FL4",
      "shell",
      "-tt",
    ]);
    // 标题带序列号：多设备时用户得看得见这块屏连的是哪台
    expect(screen.getByTestId("terminal-tabs").textContent).toContain("adb shell @ 600FL4");
    expect(screen.getByTestId("terminal-hint").textContent).toContain("18271FDF600FL4");
  });

  /** 没设备就是开不了：不许起一条 `adb -s "" shell` 之类的空会话糊弄界面。 */
  it("没有在线设备时拒绝起会话，并且一次 PTY 都不开", async () => {
    adb.devices = [{ serial: "EMULATOR36X5X9X0", state: "offline", transport: "tcp", model: "" }];
    await renderPage();
    fireEvent.click(screen.getByTestId("terminal-device-shell"));
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("没有在线设备");
    expect(fake.openArgs).toEqual([]);
  });

  /** adb 没配好就直说去设置里配，不去 PATH 里猜一个 adb（猜出来的和检测到的可能不是一条）。 */
  it("没配 adb 路径时报的是「去设置配 adb」", async () => {
    adb.env = { installed: false, path: undefined };
    await renderPage();
    fireEvent.click(screen.getByTestId("terminal-device-shell"));
    await waitFor(() => expect(screen.getByTestId("terminal-error")).toBeTruthy());
    expect(screen.getByTestId("terminal-error").textContent).toContain("设置");
    expect(fake.openArgs).toEqual([]);
  });

  /** 多设备时必须说清用了哪一台——静默挑第一台是会让人误判"我在这台上操作"的那种错。 */
  it("多台在线时说清用的是第一台、一共几台", async () => {
    adb.devices = [
      { serial: "AAA111", state: "device", transport: "usb", model: "" },
      { serial: "BBB222", state: "device", transport: "usb", model: "" },
    ];
    await renderPage();
    fireEvent.click(screen.getByTestId("terminal-device-shell"));
    await waitFor(() => expect(screen.getByTestId("terminal-hint")).toBeTruthy());
    expect(screen.getByTestId("terminal-hint").textContent).toContain("AAA111");
    expect(screen.getByTestId("terminal-hint").textContent).toContain("2");
  });

  /** R-08：离开页面才是批量收摊的地方，三条活会话各关一次，一条不留。 */
  it("离开页面把所有会话都关掉", async () => {
    const view = render(
      <I18nProvider>
        <TerminalPanel />
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
