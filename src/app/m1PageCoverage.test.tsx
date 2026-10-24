import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";
import { FilesView } from "@/features/devices/DevicesPage";

/**
 * AR9.4 · M1 页面覆盖（补齐渲染级没验到的那几个页面）。
 *
 * 与 `m1PageRegression.test.tsx` 同一目的：证明"界面真的把 typed 结果画出来了"。
 * 那份盯的是曾出事的四个界面，这份把剩下几页补上：设备列表/设备信息、Shell、
 * Logcat、ADB 转发、托管二进制、进程端口、任务中心。加解密/Frida 脚本区不在这
 * 一批的范围内（它们不碰设备契约）。
 *
 * fixture 一律按 Rust 真实返回形状写：本地模型是 camelCase（TaskDto、HostedBinary），
 * 直接透传的协议 DTO 是 snake_case（HostedRunRecord 的 log_path / started_at_unix）——
 * 这个区别本身就是 D043 修掉的东西，所以要在测试里保持可见。
 */

/** 清单 DTO 现在带着「这份是不是设备上的全貌」：测试默认给一份完整的 Agent 清单 */
const listing = (
  binaries: unknown[],
  over: Partial<{ truncated: boolean; unreadable: string[]; source: string }> = {},
) => ({ binaries, truncated: false, unreadable: [], source: "agent", ...over });

/** 目录列举的返回值现在带着「这份是不是全貌」「以谁的身份读的」（FileListing） */
const lsOf = (
  entries: unknown[],
  over: Partial<{ path: string; unreadable: string[]; truncated: boolean; source: string }> = {},
) => ({ path: "/sdcard", entries, unreadable: [], truncated: false, source: "agent", ...over });

const device = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
  info: vi.fn(),
  rootCheck: vi.fn(),
  binarySuCheck: vi.fn(),
  binaries: vi.fn(),
  hostedRuns: vi.fn(),
  binaryRun: vi.fn(),
  binaryClaim: vi.fn(),
  binaryChmod: vi.fn(),
  binaryKill: vi.fn(),
  binaryPorts: vi.fn(),
  hostedStop: vi.fn(),
  hostedList: vi.fn(),
  procPorts: vi.fn(),
  procByPort: vi.fn(),
  ls: vi.fn(),
  fileStat: vi.fn(),
  filePreview: vi.fn(),
  fsMkdir: vi.fn(),
  fsRename: vi.fn(),
  fsRemove: vi.fn(),
  fsChmod: vi.fn(),
  push: vi.fn(),
  pull: vi.fn(),
  forwardList: vi.fn(),
  forwardSetup: vi.fn(),
  ip: vi.fn(),
  // 注意：这里**没有 `shell`** —— 设备页那块原始 shell 命令腿已整条删除（真终端走 pty）
  logcat: vi.fn(),
}));

const dialog = vi.hoisted(() => ({ pickFiles: vi.fn(), pickFile: vi.fn(), pickDirectory: vi.fn() }));
/** 设备页 Shell 子标签那块真终端的传输层桩：只记参数，真 xterm 用生产那一份（jsdom 里跑得动） */
const pty = vi.hoisted(() => ({
  openCalls: [] as Array<{ program: string; args: string[] }>,
  defaultProgram: vi.fn(),
  open: vi.fn(),
  write: vi.fn(),
  resize: vi.fn(),
  close: vi.fn(),
  onExit: vi.fn(),
}));
const task = vi.hoisted(() => ({
  list: vi.fn(),
  run: vi.fn(),
  cancel: vi.fn(),
  onStatus: vi.fn(),
  onOutput: vi.fn(),
}));

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return { ...actual, deviceApi: { ...actual.deviceApi, ...device } };
});
vi.mock("@/api/pty", () => ({
  ptyApi: {
    defaultProgram: pty.defaultProgram,
    open: pty.open,
    write: pty.write,
    resize: pty.resize,
    close: pty.close,
    onExit: pty.onExit,
  },
}));
vi.mock("@/api/task", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/task")>();
  return { ...actual, taskApi: { ...actual.taskApi, ...task } };
});
vi.mock("@/app/nav", () => ({
  useAppNav: () => ({
    tab: "binary",
    subTab: "hosting",
    go: vi.fn(),
    goAdbSubTab: vi.fn(),
    pendingBinaryTab: null,
    gotoBinarySubTab: vi.fn(),
  }),
  // 用 vi.fn 是为了能在单条用例里把"这一页在不在眼前"翻过去：七页常驻，
  // 设备页的 Shell 那块屏必须同时看主 tab 与子标签才许自动开会话。
  useActiveTab: vi.fn(() => true),
}));
vi.mock("@/hooks/useDragDropPath", () => ({ useDragDropPath: vi.fn() }));
vi.mock("@/api/dialog", () => ({
  pickFiles: dialog.pickFiles,
  pickFile: dialog.pickFile,
  pickDirectory: dialog.pickDirectory,
}));
const agentMocks = vi.hoisted(() => ({
  diagnostics: vi.fn(),
  install: vi.fn(),
  restart: vi.fn(),
  probe: vi.fn(),
  onAutoChanged: vi.fn(),
}));
vi.mock("@/api/agent", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/agent")>();
  return { ...actual, agentApi: { ...actual.agentApi, ...agentMocks } };
});

import { useActiveTab } from "@/app/nav";
import { AgentSessionSection, DevicesPage } from "@/features/devices/DevicesPage";
import { BinaryHosting } from "@/features/binary/BinaryHosting";
import { BinaryPage } from "@/features/binary/BinaryPage";
import { AdbPage } from "@/features/adb/AdbPage";
import { ForwardManager } from "@/features/adb/ForwardManager";
import { ProcPorts } from "@/features/adb/ProcPorts";
import { TasksPage } from "@/features/tasks/TasksPage";

function renderPage(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>{ui}</I18nProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  Object.values(device).forEach((m) => m.mockReset());
  Object.values(task).forEach((m) => m.mockReset());
  // "这一页在不在眼前"是全局 mock 的返回值，上一条用例翻过就必须翻回来，
  // 否则后面的用例会以为自己渲染在后台页里（那种红完全指不到真问题）
  vi.mocked(useActiveTab).mockReturnValue(true);
  Object.values(pty).forEach((m) => typeof m === "function" && m.mockReset?.());
  pty.openCalls.length = 0;
  pty.defaultProgram.mockResolvedValue("/bin/zsh");
  pty.open.mockImplementation(async (args: { program: string; args: string[] }) => {
    pty.openCalls.push({ program: args.program, args: args.args });
    return {
      id: `pty-${pty.openCalls.length}`,
      pid: 4242,
      cols: (args as { cols?: number }).cols ?? 80,
      rows: (args as { rows?: number }).rows ?? 24,
      term: "xterm-256color",
    };
  });
  pty.write.mockResolvedValue(undefined);
  pty.resize.mockResolvedValue(undefined);
  pty.close.mockResolvedValue(undefined);
  pty.onExit.mockResolvedValue(() => {});
  // Agent 那组 mock 也要每条用例重来：结论条的用例要断言"点接管时不该调 install"，
  // 上一用例留下的调用记录会把它判成失败（第一版就是这么红的）。
  Object.values(agentMocks).forEach((m) => m.mockReset());
  agentMocks.onAutoChanged.mockResolvedValue(() => {});
  device.environment.mockResolvedValue({ installed: true, path: "/sdk/adb", source: "path_env" });
  device.list.mockResolvedValue([
    { serial: "PIXEL-1", state: "device", transport: "usb", model: "Pixel 6" },
  ]);
  device.onChanged.mockResolvedValue(() => {});
  device.rootCheck.mockResolvedValue(true);
  device.binarySuCheck.mockResolvedValue(true);
  // 终端启动之后那次「补登记」默认没登记上：各行仍按表外进程说实话，
  // 只有专门的用例才把它翻成 claimed:true（见下面那两条）
  device.binaryClaim.mockResolvedValue({ claimed: false, pid: null, detail: null });
  device.ip.mockResolvedValue("192.168.1.7");
  device.hostedRuns.mockResolvedValue([
    {
      handle: "0ecceabebed36d76",
      name: "frida-server",
      pid: 25265,
      // ↓ 直接透传协议 DTO：这两处必须是 snake_case，前端读的名字要跟 wire 一致
      start_time_ticks: 22_981_210,
      started_at_unix: 1_755_000_000,
      log_path: "/data/local/tmp/.toybox.run.log",
      root: false,
      state: "running",
      exit_code: null,
      detail: null,
    },
  ]);
  device.binaries.mockResolvedValue(listing([
    {
      name: "frida-server",
      path: "/data/local/tmp/frida-server",
      size: 53_539_200,
      perms: "-rwxr-xr-x",
      hasExec: true,
      // 表外同名进程：ppid=1 = 父进程已退出（守护化形状），界面据此说实话
      externalProcs: [{ pid: 31337, ppid: 1, uid: 0 }],
    },
    {
      name: "helper.dex",
      path: "/data/local/tmp/helper.dex",
      size: 12_345,
      perms: "-rw-r--r--",
      hasExec: false,
      externalProcs: [],
    },
  ]));
  device.procPorts.mockResolvedValue([
    { address: "0100007F:6978", port: 27000, listen: true, family: "tcp" },
  ]);
  device.procByPort.mockResolvedValue([{ pid: 25265, name: "frida-server" }]);
  device.forwardList.mockResolvedValue([
    { serial: "PIXEL-1", local: "tcp:27042", remote: "tcp:27042" },
  ]);
  device.binaryPorts.mockResolvedValue([{ address: "", port: 27042, listen: true, family: "tcp" }]);
  task.list.mockResolvedValue([
    {
      id: "task-1",
      taskType: "adb.shell",
      name: "shell: getprop ro.build.version.sdk",
      status: "success",
      exitCode: 0,
      createdAt: 1_755_000_000,
      finishedAt: 1_755_000_002,
    },
    {
      id: "task-2",
      taskType: "adb.install",
      name: "install base.apk",
      status: "failed",
      exitCode: 1,
      createdAt: 1_755_000_100,
      finishedAt: 1_755_000_120,
    },
  ]);
  task.onStatus.mockResolvedValue(() => {});
  task.onOutput.mockResolvedValue(() => {});
});

describe("设备页（列表 / 信息 / Shell / Logcat）", () => {
  it("设备列表与设备信息用的是同一份 typed 数据", async () => {
    device.info.mockResolvedValue({
      model: "Pixel 6",
      manufacturer: "Google",
      androidVersion: "14",
      sdkInt: "34",
      ip: "192.168.1.7",
    });
    renderPage(<DevicesPage />);
    expect(await screen.findByText("Pixel 6")).toBeTruthy();
    fireEvent.click(await screen.findByRole("tab", { name: "设备信息" }));
    await waitFor(() => expect(device.info).toHaveBeenCalledWith("PIXEL-1"));
    expect(document.body.textContent).toContain("Google");
    expect(document.body.textContent).toContain("192.168.1.7");
  });

  /**
   * 设备页的 Shell 子标签**就是那块真终端**（第八十八轮：旧"命令框 + 回车执行一条 adb shell"
   * 整体撤掉，点它只显示 shell 终端）。判据按交给 PTY 的参数数组原样钉：
   * `adb -s <她选中的那台> shell -tt`——少那个 `-tt` 就没有远端 PTY，
   * Ctrl-C 中断、行编辑、top/vi 这类全屏程序全都不成立（五条实测见 `lib/deviceShell`）。
   *
   * 同时钉住三件相邻的事：
   * - 停在别的子标签时**一块屏都不开**（隐藏的页偷偷起进程 = 她看不见也关不掉）；
   * - 旧的命令框与那句"ADB 原始会话（不经 Agent）"的说明**不许留着**（撤功能不许留半个）；
   * - Logcat 那条**没被顺手搬走**，仍是原始任务链路。
   */
  it("设备页 Shell 子标签只有一块真终端，开的是 adb -s <选中的那台> shell -tt", async () => {
    renderPage(<DevicesPage />);
    await screen.findByText("Pixel 6");
    // 子标签内容是一直挂着的（切走不该杀会话），所以"面板活着"不能当证据：
    // 要等到懒加载那块面板**已经能自动开屏**之后再等一会儿，才说明"没在看它"真的没起进程。
    // （上一版这里只等了 20 ms、面板还在解析 chunk，撤掉 `visible` 那道闸都测不出来——撤闸验证当场把它抓了。）
    await waitFor(() => expect(screen.getByTestId("terminal-tabs")).toBeTruthy());
    await new Promise((resolve) => setTimeout(resolve, 120));
    // 默认在「设备列表」：这块屏还没被看到，一条进程都不该起
    expect(pty.openCalls).toEqual([]);

    // ⚠️ 这里必须用 `userEvent`：Radix 的子标签**靠聚焦切换**，`fireEvent.click` 不带焦点，
    // 于是"点了 Shell"其实一页都没换——测试会以为自己点了个不存在的入口（第一版就是这么红的）。
    await userEvent.click(await screen.findByRole("tab", { name: "Shell" }));
    await waitFor(() => expect(screen.getByTestId("terminal-chip-dev-1")).toBeTruthy());
    await waitFor(() => expect(pty.openCalls.length).toBe(1));
    expect(pty.openCalls[0]).toEqual({
      program: "/sdk/adb",
      args: ["-s", "PIXEL-1", "shell", "-tt"],
    });
    // 会话开在设备页自己那张表里（dev-1），名字仍是「会话 N」
    const shellPane = within(screen.getByTestId("terminal-tabs"));
    expect(shellPane.getByTestId("terminal-chip-dev-1").textContent).toContain("会话 1");

    // 旧形状不许留下痕迹：命令框、占位提示、那句边界说明，一个都不在
    expect(screen.queryByPlaceholderText(/input text hello/)).toBeNull();
    expect(document.body.textContent).not.toContain("不经 Agent");

    // Logcat 还在原地，走的仍是它自己那条 command（这次搬走的不该顺手带走它）
    device.logcat.mockResolvedValue("task-2");
    await userEvent.click(screen.getByRole("tab", { name: "Logcat" }));
    const filter = await screen.findByPlaceholderText(/如 crash/);
    fireEvent.change(filter, { target: { value: "ActivityManager" } });
    fireEvent.keyDown(filter, { key: "Enter" });
    await waitFor(() => expect(device.logcat).toHaveBeenCalledWith("PIXEL-1", "ActivityManager"));
    // 起了一条设备会话之后，Logcat 不该再多起屏
    expect(pty.openCalls.length).toBe(1);
  });

  /**
   * 同一件事的另一半：**主 tab 不在设备页时**，哪怕子标签停在 Shell 上也不许起进程。
   * 七页是常驻的（切主 tab 只改 visibility），只看子标签就会出现"她在别的页上，
   * 这里偷偷开了一条设备 shell"——看不见，也就关不掉。
   */
  it("设备页不在眼前时，Shell 子标签一块屏都不开；切回来才开", async () => {
    // 这一条要**重渲染同一个树**（切主 tab 在真窗口里就是一次重渲染），所以不用 renderPage：
    // 它会把 provider 一起换掉，rerender 只喂 `<DevicesPage />` 就把 QueryClient/I18n 都丢了。
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const tree = () => (
      <QueryClientProvider client={qc}>
        <I18nProvider>
          <DevicesPage />
        </I18nProvider>
      </QueryClientProvider>
    );
    vi.mocked(useActiveTab).mockReturnValue(false);
    const view = render(tree());
    await waitFor(() => expect(screen.getByTestId("terminal-tabs")).toBeTruthy());
    await userEvent.click(await screen.findByRole("tab", { name: "Shell" }));
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(pty.openCalls).toEqual([]);

    // 她把主 tab 切回设备页（子标签还停在 Shell）：这一下才该有会话 1。
    // 改完 mock 必须**真的重渲染一次**——返回值变了不会自己触发渲染，
    // 少了这一行就会得到"切回来还是不开"的假红（第一版就是这么红的）。
    vi.mocked(useActiveTab).mockReturnValue(true);
    view.rerender(tree());
    await waitFor(() => expect(pty.openCalls.length).toBe(1));
    expect(pty.openCalls[0].args).toEqual(["-s", "PIXEL-1", "shell", "-tt"]);
  });
});

describe("ADB 子页（转发 / 托管 / 端口）", () => {
  it("读回设备上的真实转发规则，并按规则验证一行", async () => {
    renderPage(<ForwardManager />);
    await screen.findByText("-s PIXEL-1");
    // ① 「当前生效规则」直接来自 forwardList 的返回，界面不自己拼
    fireEvent.click(await screen.findByText("当前生效规则"));
    await waitFor(() => expect(device.forwardList).toHaveBeenCalledWith("PIXEL-1"));
    await waitFor(() =>
      expect(document.body.textContent).toContain("tcp:27042 → tcp:27042"),
    );

    // ② 单行验证要先两边都填（只填一边按钮是 disabled 的，这是界面上的真规则）
    fireEvent.change((await screen.findAllByPlaceholderText("8080"))[0], {
      target: { value: "27042" },
    });
    fireEvent.change((await screen.findAllByPlaceholderText("tcp:8080"))[0], {
      target: { value: "27042" },
    });
    fireEvent.click((await screen.findAllByText("验证"))[0]);
    await waitFor(() => expect(document.body.textContent).toContain("生效"));
  });

  it("托管入口挂在「二进制」主 tab 下，ADB 页不再出现它（搬完别留半个）", async () => {
    // 这一条钉的是"位置"：托管管的是"跑哪个二进制"，与 so 替换同类；
    // 端口转发/进程端口才是 ADB 那页的事。位置飘走时 frida 的修复深链也会指错页，
    // 所以这里同时盯住 AdbPage 不许把 tab 加回来。
    renderPage(<BinaryPage />);
    expect(await screen.findByRole("tab", { name: "二进制托管" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "so 替换" })).toBeTruthy();
    renderPage(<AdbPage />);
    await screen.findByRole("tab", { name: "端口转发" });
    expect(screen.queryAllByRole("tab", { name: "二进制托管" }).length).toBe(1);
  });

  it("托管页用协议 DTO 对账：运行中的 pid 与可停止状态来自 hosted.runs", async () => {
    renderPage(<BinaryHosting />);
    await waitFor(() => expect(device.binaries).toHaveBeenCalled());
    // 本地模型（camelCase）：权限串决定绿/红与能否双击入区。
    // 权限现在渲染成分色胶囊（类型字符 + 权限段两个 span），整串仍要能在行里读到：
    // `screen.getByText` 的默认匹配不跨元素，这里按容器取文本再比。
    const permsCell = await screen.findByTestId("bin-perms-frida-server");
    expect(permsCell.textContent).toBe("-rwxr-xr-x");
    // 协议 DTO（snake_case：start_time_ticks / started_at_unix / handle / state）
    // 决定托管区里这一行是不是「在跑、pid 多少、能不能按句柄停止」。
    // 曾经这里读的是 startedAtUnix → undefined，行就永远显示未运行。
    await waitFor(() => expect(device.hostedRuns).toHaveBeenCalled());
    expect(await screen.findByText(/25265/)).toBeTruthy();
    expect(await screen.findByText("终止")).toBeTruthy();
    /*
     * 托管执行那一行是**纵向堆叠**：文件名 / 动作 / 参数回执 / 端口各占一行。
     *
     * 这条断言防的是一个真实炸过的回归：换成气泡行时用了 `multiline`，而它只做顶部对齐、
     * 容器仍是横向 flex，于是每个块变成一个 row 子项、被压到最小宽度，中文逐字换行成
     * 竖排（「这一次是用这些参数起的：…」整列竖着，用户截图报的严重显示问题）。
     * jsdom 不编译 Tailwind，只能断言类名——但它正是"排布方向"的唯一来源，够用。
     */
    const hostedRow = screen.getByTestId("hosted-frida-server");
    expect(hostedRow.firstElementChild?.className).toContain("flex-col");
    // 关键：设备说"在跑"时「终止」必须是**可点**的。曾经 running 一个字段兼作两用
    // （设备在跑 / 我的指令在途），于是按钮 disabled={row.running} 把在跑的行的终止
    // 自己禁成灰色 —— 用户报的"发现托管进程在跑但停不掉"就是它。
    // 可执行状态就着名字上色（绿=可执行、红=不可执行）；「外部在跑」另给一枚黄色徽章
    expect(
      within(screen.getByTestId("bin-frida-server")).getByText("frida-server").className,
    ).toContain("text-emerald-600");
    expect(within(screen.getByTestId("bin-helper.dex")).getByText("helper.dex").className).toContain(
      "text-red-500",
    );
    // 入口归属：清单行只管"加入托管"(+)，启动入口在托管执行那一行——
    // 原来那枚「执行」被终端按钮替掉了，两件事不再混在一个列表行里。
    const add = screen.getByTestId("add-frida-server");
    expect(add.closest("li")).toBe(screen.getByTestId("bin-frida-server"));
    expect(add).toBeEnabled();
    expect(screen.getByTestId("add-helper.dex")).toBeDisabled(); // 没执行位的文件不给启动
    expect(screen.queryByTestId("interactive-frida-server")).toBeNull();
    // 设备说这一行"在跑"，就不该再给启动入口（终端启动也一样：不给同一个文件起第二个实例）
    expect(screen.queryByTestId("hosted-terminal-frida-server")).toBeNull();
    expect(screen.getByTestId("kill-frida-server")).toBeEnabled();
    const external = screen.getByTestId("external-frida-server");
    expect(external.className).toContain("bg-amber-500/10");
    expect(external.className).toContain("text-amber-500");

    // 备注归文件列表那一行，而且**平时只是文字**：点"加备注"才出现输入框（不是每行挂着表单）
    for (const el of screen.getAllByTestId(/^note-/)) {
      expect(el.closest("li")).toHaveAttribute("data-testid", expect.stringMatching(/^bin-/));
    }
    expect(screen.queryByTestId("note-frida-server")).toBeNull();
    fireEvent.click(screen.getByTestId("note-add-frida-server"));
    const noteInput = await screen.findByTestId("note-frida-server");
    // 快捷备注曾经点了没反应：输入框的无条件 onBlur 会在鼠标落到下拉前就把编辑行卸载掉。
    // 判据就是这条 —— 焦点移到下拉时不许收起，移到别处才收起。
    const quick = screen.getByTestId("note-quick-frida-server");
    fireEvent.blur(noteInput, { relatedTarget: quick });
    expect(screen.getByTestId("note-frida-server")).toBeInTheDocument();
    // 预设真的写进去了，并且写完就收起（此时那一行显示备注文字 + 铅笔）
    fireEvent.change(quick, { target: { value: "frida server" } });
    expect(await screen.findByText("frida server")).toBeInTheDocument();
    expect(screen.queryByTestId("note-frida-server")).toBeNull();
    const again = screen.getByTestId("note-edit-frida-server");
    fireEvent.click(again);
    const second = await screen.findByTestId("note-frida-server");
    // 焦点移到别处才收起
    fireEvent.blur(second, { relatedTarget: document.body });
    expect(screen.queryByTestId("note-frida-server")).toBeNull();
    expect(screen.getByTestId("kill-frida-server")).toBeEnabled();
    expect(screen.queryByTestId("hosted-terminal-frida-server")).toBeNull(); // 设备说在跑，就不给启动入口
    // 真点一下：按钮必须是活的，按 handle 停止要真的被调用
    device.hostedStop.mockResolvedValue({
      record: {
        handle: "0ecceabebed36d76",
        name: "frida-server",
        pid: 25265,
        start_time_ticks: 22_981_210,
        started_at_unix: 1_755_000_000,
        log_path: "/data/local/tmp/.frida-server.run.log",
        root: false,
        state: "exited",
        exit_code: 0,
        detail: null,
      },
      outcome: "signaled",
      identity_verified: true,
      record_dropped: true,
    });
    fireEvent.click(screen.getByTestId("kill-frida-server"));
    await waitFor(() =>
      expect(device.hostedStop).toHaveBeenCalledWith("PIXEL-1", "0ecceabebed36d76", 25265),
    );
    // 按 handle + start time 停止那段真实链路在 AR7.3 真机腿里验
    // （real_agent_hosted_lifecycle_handles_identity_and_reaping），
    // 页面测试不重复假装覆盖了它。
  });

  it("已经在跑就只给一个「停止进程」按钮：确认后才动手，且绝不顺手起一个", async () => {
    // 用户的口径就是这条：启动前先查在不在跑；在跑就给一个杀死进程的按钮。
    // 运行表清空，模拟"软件后启动、这个实例不是我们起的"。
    device.hostedRuns.mockResolvedValue([]);
    device.binaryRun.mockResolvedValue({
      pid: 31337,
      started: false,
      detail: "frida-server 已经在跑（pid 31337 · 父进程已退出 · root），不在本工具的托管表里",
    });
    device.binaryKill.mockResolvedValue(undefined);
    renderPage(<BinaryHosting />);
    expect(await screen.findByTestId("external-frida-server")).toBeTruthy();
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    const hosted = await screen.findByTestId("hosted-frida-server");
    expect(hosted.textContent ?? "").toContain("已在运行 pid 31337");
    // 关键反向断言：这一行**没有**「执行」按钮 —— 点了也只会起一个秒退的进程
    expect(screen.queryByTestId("hosted-terminal-frida-server")).toBeNull();
    device.binaryRun.mockClear();
    device.binaryKill.mockClear();
    // 停止后清单会重新拉一次：这次报告"没有外部实例了"，模拟进程真被停掉
    device.binaries.mockResolvedValue(listing([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]));
    fireEvent.click(screen.getByTestId("stop-external-frida-server"));
    // 不可逆操作先确认一次，不直接杀
    expect(device.binaryKill).not.toHaveBeenCalled();
    const note = await screen.findByTestId("external-note-frida-server");
    expect(note.textContent ?? "").toContain("31337");
    // 文案不能撒谎：无句柄 ≠ 停不掉（停止按钮走的就是按身份核验的终止通道）
    expect(note.textContent ?? "").not.toContain("停不掉");
    fireEvent.click(screen.getByTestId("confirm-stop-frida-server"));
    // root=true：目标可能是 su 起的（shell 杀不动）；带上名字，设备端先核身份再发信号
    await waitFor(() =>
      expect(device.binaryKill).toHaveBeenCalledWith("PIXEL-1", 31337, true, "frida-server"),
    );
    expect(device.binaryRun).not.toHaveBeenCalled();
    // 停下来之后，同一行才重新给出启动入口
    await waitFor(() => expect(screen.getByTestId("hosted-terminal-frida-server")).toBeTruthy());
  });

  it("点「终端启动」走的是终端：开一块 PTY 屏，不再打 device_binary_run", async () => {
    // 启动方式换了：不再由 Agent 后台起 + 日志文件轮询，而是在本页右侧那块真终端里跑。
    // 判据两头都要钉：新链路确实起了 PTY（参数里带 ./文件名），老链路一次都没被碰。
    device.binaries.mockResolvedValue(listing([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]));
    device.hostedRuns.mockResolvedValue([]);
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    const row = await screen.findByTestId("hosted-frida-server");
    fireEvent.click(row.querySelector("[data-testid='hosted-terminal-frida-server']") as Element);
    await waitFor(() => expect(pty.open).toHaveBeenCalled());
    const openedArgs = vi.mocked(pty.open).mock.calls.at(-1)?.[0].args ?? [];
    expect(openedArgs).toContain("-tt");
    // 名字必须带 ./（裸文件名会被设备 shell 按 PATH 找）；引用写法随 Root 开关变，所以只认文件名
    expect(openedArgs.at(-1)).toContain("./frida-server");
    expect(device.binaryRun).not.toHaveBeenCalled();
    expect(screen.queryByTestId(/^err-/)).toBeNull(); // 换启动方式不是失败，别报红
  });

  it("终端起的那次执行会补登记：设备运行表收下之后，行按表显示 pid 与「终止」", async () => {
    // 用户报的现场：明明是在这一页点起来的，行上却说"它不在本工具的托管表里"。
    // 根因是终端那次 exec 走 adb shell、不经过 Agent，表里自然没有记录。
    // 现在开屏之后补一次认领：桌面只交候选 pid，认不认由设备侧实证判。
    device.binaries.mockResolvedValue(listing([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]));
    let runs: unknown[] = [];
    device.hostedRuns.mockImplementation(async () => runs);
    let resolveClaim: (value: unknown) => void = () => {};
    device.binaryClaim.mockReturnValue(
      new Promise((resolve) => {
        resolveClaim = resolve;
      }),
    );
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    const row = await screen.findByTestId("hosted-frida-server");
    fireEvent.click(row.querySelector("[data-testid='hosted-terminal-frida-server']") as Element);
    await waitFor(() => expect(device.binaryClaim).toHaveBeenCalled());
    // 参数：本设备的序列号、这个文件、启动用的参数、以及开屏前就在跑的同名 pid（这里没有）
    expect(device.binaryClaim.mock.calls.at(-1)?.slice(0, 4)).toEqual([
      "PIXEL-1",
      "frida-server",
      [],
      [],
    ]);
    // 认领在途时不许出现"可能不是你起的"那种猜测文案，也不该已经有 pid
    expect(row.textContent ?? "").not.toContain("不在本工具的托管表里");
    expect(screen.queryByTestId("pid-frida-server")).toBeNull();
    // 设备把这条收进表了：行的 pid/终止全部来自运行表，不是桌面自己记的
    runs = [
      {
        handle: "aa55bb66cc77dd88",
        name: "frida-server",
        pid: 24208,
        start_time_ticks: 22_981_210,
        started_at_unix: 1_755_000_000,
        log_path: "/data/local/tmp/.frida-server.run.log",
        root: false,
        state: "running",
        exit_code: null,
        detail: "adopted",
      },
    ];
    resolveClaim({ claimed: true, pid: 24208, detail: null });
    await waitFor(() =>
      expect(screen.getByTestId("pid-frida-server").textContent ?? "").toContain("24208"),
    );
    expect(await screen.findByTestId("kill-frida-server")).toBeEnabled();
    expect(screen.queryByTestId("external-note-frida-server")).toBeNull();
    // 「终止」把设备上的进程停掉之后，为它开的那块屏也得收掉：
    // 进程早没了、屏还亮着一段跑完的输出，看上去就是"没停干净"。
    device.hostedStop.mockResolvedValue({
      record: {
        handle: "aa55bb66cc77dd88",
        name: "frida-server",
        pid: 24208,
        start_time_ticks: 22_981_210,
        started_at_unix: 1_755_000_000,
        log_path: "/data/local/tmp/.frida-server.run.log",
        root: false,
        state: "exited",
        exit_code: null,
        detail: null,
      },
      outcome: "signaled",
      identity_verified: true,
      record_dropped: true,
    });
    fireEvent.click(screen.getByTestId("kill-frida-server"));
    await waitFor(() =>
      expect(device.hostedStop).toHaveBeenCalledWith("PIXEL-1", "aa55bb66cc77dd88", 24208),
    );
    await waitFor(() => expect(pty.close).toHaveBeenCalledWith("pty-1"));
    // 结果播报走**中性那一格**（红色那格只留给真错误：旧版"停止成功"也标红，看着像失败）
    await waitFor(() =>
      expect(screen.getByTestId("action-note-frida-server").textContent ?? "").toContain(
        "终端屏也一起关掉了",
      ),
    );
    expect(screen.queryByTestId("err-frida-server")).toBeNull();
    // 设备表里这条已经没了 ⇒ 行回到"未运行"，启动入口重新出现
    runs = [];
    fireEvent.click(screen.getByTestId("hosted-terminal-frida-server"));
    await waitFor(() => expect(screen.queryByTestId("pid-frida-server")).toBeNull());
    // 用户报的错显就在这一步：停完再启动，行上既不该还挂着上一次"已终止 pid …"那格播报，
    // 也不该在根本没查过端口之前印一句"无 LISTEN 端口"（那是把没发生的查询当证据）
    await waitFor(() => expect(screen.queryByTestId("action-note-frida-server")).toBeNull());
    expect(screen.queryByTestId("err-frida-server")).toBeNull();
    expect(screen.queryByTestId("ports-frida-server")).toBeNull();
    expect(row.textContent ?? "").not.toContain("无 LISTEN 端口");
    // 也没有第二套输入通道：stdin 就在那块屏里，行上不许再冒"输入通道"这种话
    expect(row.textContent ?? "").not.toContain("输入通道");
  });

  it("托管行布局：文件名独立一行，pid 到探测帮助在下一行", async () => {
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    const row = await screen.findByTestId("hosted-frida-server");
    const chip = row.querySelector("[data-testid='cmd-frida-server']") as Element;
    const actions = row.querySelector(
      "[data-testid='hosted-actions-frida-server']",
    ) as HTMLElement;
    expect(chip).toBeTruthy();
    expect(actions).toBeTruthy();
    // 名字不许挤进动作那一行：一行里塞四五个按钮，长文件名就会被截断（用户要的就是分开）
    expect(chip.closest("[data-testid='hosted-actions-frida-server']")).toBeNull();
    // pid / 终止 / 探测帮助都归第二行
    for (const id of ["pid-frida-server", "kill-frida-server", "probe-frida-server"]) {
      expect(actions.querySelector(`[data-testid='${id}']`), `${id} 该在动作那一行`).toBeTruthy();
    }
  });

  it("端口那块只说查过的事：查失败就说查失败，不许默认「无 LISTEN 端口」", async () => {
    // 用户报的第三条错显就是这句：进程重新起来后，谁都没查过端口，行上先印了"无 LISTEN 端口"。
    device.binaryPorts.mockRejectedValue(new Error("设备端读不到 /proc/net/tcp"));
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    // 设备运行表说 pid 25265 在跑 ⇒ 这一行自己去查一次（老「执行」那条路本来会自动查）
    await waitFor(() =>
      expect(device.binaryPorts).toHaveBeenCalledWith("PIXEL-1", 25265, false),
    );
    await waitFor(() =>
      expect(screen.getByTestId("ports-error-frida-server").textContent ?? "").toContain(
        "端口没查到",
      ),
    );
    expect(screen.getByTestId("ports-frida-server").textContent ?? "").not.toContain("无 LISTEN");
  });

  it("确实查过、真的没有监听端口，才说「无 LISTEN 端口」", async () => {
    device.binaryPorts.mockResolvedValue([]);
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    await waitFor(() =>
      expect(screen.getByTestId("ports-frida-server").textContent ?? "").toContain(
        "无 LISTEN 端口",
      ),
    );
  });

  it("Root 开关开着时，终端启动按 root 起，登记也按 root 报", async () => {
    // 页顶那句"以 root 执行 chmod/启动/终止"以前对启动是空话：终端那条路一直按普通执行起。
    // 现在开关说了算：远端命令套 su -c，认领时也把 root 一并交给设备侧核对属主。
    device.binaries.mockResolvedValue(listing([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]));
    device.hostedRuns.mockResolvedValue([]);
    device.binaryClaim.mockResolvedValue({ claimed: false, pid: null, detail: null });
    renderPage(<BinaryHosting />);
    const box = (await screen.findByRole("checkbox", { name: "Root 执行" })) as HTMLInputElement;
    await waitFor(() => expect(box.checked).toBe(true)); // su 探得过 → 默认开
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    const row = await screen.findByTestId("hosted-frida-server");
    fireEvent.click(row.querySelector("[data-testid='hosted-terminal-frida-server']") as Element);
    await waitFor(() => expect(pty.open).toHaveBeenCalled());
    const remote = vi.mocked(pty.open).mock.calls.at(-1)?.[0].args.at(-1) ?? "";
    expect(remote).toContain("su -c");
    expect(remote).toContain("./frida-server");
    await waitFor(() => expect(device.binaryClaim).toHaveBeenCalled());
    expect(device.binaryClaim.mock.calls.at(-1)?.[4]).toBe(true);
  });

  it("pid 只由设备运行表支撑：在册的那行显示 pid 并给「终止」", async () => {
    // AR7.7 的要点没变，来源变了：pid 不再是"桌面点过一次执行"的记忆，
    // 而是设备侧 hosted.runs 回读的记录 —— 所以界面也不再有"未登记"那种半截状态。
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    await screen.findByTestId("hosted-frida-server");
    expect((await screen.findByTestId("pid-frida-server")).textContent ?? "").toContain("25265");
    expect(await screen.findByTestId("kill-frida-server")).toBeEnabled();
    expect(screen.queryByTestId("hosted-terminal-frida-server")).toBeNull(); // 在跑就不给再启动
  });

  it("这台机手动关过就不再自动开、也不再探测（非 root 手机不该每次被试）", async () => {
    localStorage.clear();
    localStorage.setItem("adb.binary.root.PIXEL-1", "0");
    device.binarySuCheck.mockClear();
    renderPage(<BinaryHosting />);
    const box = (await screen.findByRole("checkbox", { name: "Root 执行" })) as HTMLInputElement;
    expect(box.checked).toBe(false);
    await new Promise((r) => setTimeout(r, 30));
    expect(device.binarySuCheck).not.toHaveBeenCalled();
    localStorage.clear();
  });

  it("上传文件：推到托管目录、等任务终态、缺执行位补权限，被拒的要给原因", async () => {
    const hosted = [
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rw-r--r--", // 刚 push 上去的样子：没有执行位
        hasExec: false,
        externalProcs: [],
      },
    ];
    device.binaries.mockResolvedValue(listing(hosted));
    dialog.pickFiles.mockResolvedValue([
      "/Users/x/Downloads/frida-server",
      "/Users/x/Downloads/中文",
    ]);
    device.push.mockResolvedValue("task-push-1");
    task.list.mockResolvedValue([
      {
        id: "task-push-1",
        taskType: "adb.push",
        name: "adb push",
        status: "success",
        exitCode: 0,
        createdAt: 0,
        finishedAt: 1,
      },
    ]);
    device.binaryChmod.mockResolvedValue(undefined);
    // 设备回读的实际权限才是"收紧成功"的依据（/sdcard 那类 FUSE 会吃掉某些位）
    device.fsChmod.mockResolvedValue({
      path: "/data/local/tmp/frida-server",
      mode: 0o755,
      mode_text: "-rwxr-xr-x",
      previous_mode: 0o666,
      previous_mode_text: "-rw-rw-rw-",
      verified: true,
    });
    renderPage(<BinaryHosting />);
    const btn = await screen.findByTestId("upload-binary");
    // 设备是异步选上的：没 serial 时这个按钮是 disabled，点了等于没点（别测空气）
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);
    // 只推合法名字那一个，目标路径钉死在托管目录
    await waitFor(() =>
      expect(device.push).toHaveBeenCalledWith(
        "PIXEL-1",
        "/Users/x/Downloads/frida-server",
        "/data/local/tmp/frida-server",
      ),
    );
    expect(device.push).toHaveBeenCalledTimes(1);
    // 落地即收成 0755（走 filesystem.chmod，带精确模式），不再只"加执行位"
    await waitFor(() =>
      expect(device.fsChmod).toHaveBeenCalledWith(
        "PIXEL-1",
        "/data/local/tmp/frida-server",
        0o755,
        // 第四位是这台设备的 root 意图（与文件页共用同一份偏好）
        expect.any(Boolean),
      ),
    );
    expect(device.binaryChmod).not.toHaveBeenCalled();
    const notice = await screen.findByTestId("binary-notice");
    // 权限显示的是**设备回读值**，不是我们传进去的那个数
    expect(notice.textContent ?? "").toContain("-rwxr-xr-x");
    // 同名文件已在托管目录里 → 必须说成覆盖，不能装作是新文件
    expect(notice.textContent ?? "").toContain("覆盖");
    // 被白名单拒掉的那个必须带原因：静默丢文件是这类入口最坏的写法
    expect(notice.textContent ?? "").toContain("白名单");
  });

  it("这台手机上已经没有这个文件了 → 它的本机记录随之删掉（每台手机各管各的）", async () => {
    // 用户口径：清单必须是从这台手机读来的，本机只留备注/参数，且按设备号对应；
    // 手机自己把文件删了，记录就不该继续躺着。判据见 listingIsTrustworthy。
    localStorage.clear();
    localStorage.setItem("adb.binary.note.PIXEL-1.gone.bin", "旧备注");
    localStorage.setItem("adb.binary.launch.PIXEL-1.gone.bin.args", "-h");
    localStorage.setItem("adb.binary.note.OTHER-9.gone.bin", "另一台手机的同名记录");
    // 删之前必须逐个回问设备：只有它明确回答"没有这个文件"才算被删
    device.fileStat.mockRejectedValue({
      code: "NOT_FOUND",
      message: "设备上没有这个文件或目录：/data/local/tmp/gone.bin",
    });
    renderPage(<BinaryHosting />);
    await waitFor(() =>
      expect(localStorage.getItem("adb.binary.note.PIXEL-1.gone.bin")).toBeNull(),
    );
    expect(localStorage.getItem("adb.binary.launch.PIXEL-1.gone.bin.args")).toBeNull();
    // 别的设备一条不碰：序列号就是这些记录的户口
    expect(localStorage.getItem("adb.binary.note.OTHER-9.gone.bin")).toBe("另一台手机的同名记录");
    const notice = await screen.findByTestId("binary-notice");
    // 删了用户的数据就必须交代：几条、为什么
    expect(notice.textContent ?? "").toContain("清掉本机记录 2 条");
    expect(device.fileStat).toHaveBeenCalledWith("PIXEL-1", "/data/local/tmp/gone.bin");
    localStorage.clear();
  });

  it("文件其实还在（清单与推送撞了几秒钟）：一条记录都不许删", async () => {
    localStorage.clear();
    localStorage.setItem("adb.binary.note.PIXEL-1.gone.bin", "旧备注");
    device.fileStat.mockResolvedValue({
      requested_path: "/data/local/tmp/gone.bin",
      path: "/data/local/tmp/gone.bin",
      stat: {
        name: "gone.bin",
        kind: "file",
        mode: 0o755,
        mode_text: "-rwxr-xr-x",
        uid: 2000,
        gid: 2000,
        size: 10,
        mtime_unix: 1,
        readable: true,
      },
    });
    renderPage(<BinaryHosting />);
    await waitFor(() => expect(device.fileStat).toHaveBeenCalled());
    // 设备说"在这儿呢"：本机记录留着，也不许冒出"清掉了几条"这种话
    expect(localStorage.getItem("adb.binary.note.PIXEL-1.gone.bin")).toBe("旧备注");
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("binary-notice")?.textContent ?? "").not.toContain("清掉本机记录");
    localStorage.clear();
  });

  it("清单被设备端截断：说破「不是目录全貌」，本机记录一条都不删", async () => {
    localStorage.clear();
    localStorage.setItem("adb.binary.note.PIXEL-1.gone.bin", "旧备注");
    device.fileStat.mockRejectedValue({ code: "NOT_FOUND", message: "没有" });
    device.binaries.mockResolvedValue(
      listing([{ name: "frida-server", path: "/data/local/tmp/frida-server", size: 1, perms: "-rwxr-xr-x", hasExec: true, externalProcs: [] }], { truncated: true }),
    );
    renderPage(<BinaryHosting />);
    await waitFor(() => expect(screen.getByTestId("binaries-truncated")).toBeTruthy());
    // 清单不完整时连"回问设备"都不该发起：它没有资格怀疑任何文件被删了
    expect(device.fileStat).not.toHaveBeenCalled();
    expect(localStorage.getItem("adb.binary.note.PIXEL-1.gone.bin")).toBe("旧备注");
    localStorage.clear();
  });

  it("设备不在 adb 列表里：不是故障，给的是下一步", async () => {
    device.binaries.mockRejectedValue({
      code: "DEVICE_OFFLINE",
      message: "设备当前不在这台 adb 的列表里：adb: device 'PIXEL-1' not found",
    });
    renderPage(<BinaryHosting />);
    const err = await screen.findByTestId("binaries-error");
    expect(err.textContent ?? "").toContain("不在这台 adb 的列表里");
    expect(err.textContent ?? "").not.toContain("内部错误");
    expect(err.textContent ?? "").toContain("允许调试");
  });

  it("收不成 0755 时退回只补执行位，并且明说「组/其他仍可写」的风险", async () => {
    device.binaries.mockResolvedValue(listing([
      {
        name: "newbin",
        path: "/data/local/tmp/newbin",
        size: 100,
        perms: "-rw-rw-rw-",
        hasExec: false,
        externalProcs: [],
      },
    ]));
    dialog.pickFiles.mockResolvedValue(["/Users/x/Downloads/newbin"]);
    device.push.mockResolvedValue("task-loose-1");
    task.list.mockResolvedValue([
      { id: "task-loose-1", status: "success", exitCode: 0, finishedAt: 1 },
    ]);
    // FUSE/SELinux 一类会把位吃掉：回读说没收成，界面就不能说"已收紧"
    device.fsChmod.mockResolvedValue({
      path: "/data/local/tmp/newbin",
      mode: 0o777,
      mode_text: "-rwxrwxrwx",
      previous_mode: 0o666,
      previous_mode_text: "-rw-rw-rw-",
      verified: true,
    });
    device.binaryChmod.mockClear();
    renderPage(<BinaryHosting />);
    const btn = await screen.findByTestId("upload-binary");
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);
    const notice = await screen.findByTestId("binary-notice");
    await waitFor(() => expect(notice.textContent ?? "").toContain("仍可写"));
    expect(notice.textContent ?? "").toContain("-rwxrwxrwx");
  });

  it("上传任务没跑完就照实说，不报成功、也不去补权限", async () => {
    device.binaries.mockResolvedValue(listing([]));
    dialog.pickFiles.mockResolvedValue(["/Users/x/Downloads/newbin"]);
    device.push.mockResolvedValue("task-push-2");
    task.list.mockResolvedValue([
      {
        id: "task-push-2",
        taskType: "adb.push",
        name: "adb push",
        status: "failed",
        exitCode: 1,
        createdAt: 0,
        finishedAt: 1,
      },
    ]);
    device.binaryChmod.mockClear();
    renderPage(<BinaryHosting />);
    const btn2 = await screen.findByTestId("upload-binary");
    await waitFor(() => expect(btn2).toBeEnabled());
    fireEvent.click(btn2);
    const notice = await screen.findByTestId("binary-notice");
    await waitFor(() => expect(notice.textContent ?? "").toContain("上传未完成"));
    expect(notice.textContent ?? "").toContain("failed");
    expect(device.binaryChmod).not.toHaveBeenCalled();
  });

  it("没选设备时上传按钮必须是灰的，而且灰得有理由", async () => {
    device.list.mockResolvedValue([]);
    renderPage(<BinaryHosting />);
    const btn = await screen.findByTestId("upload-binary");
    expect(btn).toBeDisabled();
    expect(btn.getAttribute("title")).toContain("要先选一台设备");
    device.list.mockResolvedValue([
      { serial: "PIXEL-1", state: "device", transport: "usb", model: "Pixel 6" },
    ]);
  });

  it("拿不到设备运行表时不拿桌面记忆冒充：没有 pid chip，也不说未登记", async () => {
    // 这条是 AR7.7 原则在新链路下的版本：pid 的唯一来源是设备回读的运行表。
    // 启动方式换成终端以后，界面更没有"我记得我起过"这种状态了 —— 读不到就是不说它在跑。
    device.hostedRuns.mockRejectedValue(new Error("Agent 未在线"));
    device.binaries.mockResolvedValue(listing([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]));
    renderPage(<BinaryHosting />);
    fireEvent.doubleClick((await screen.findByTestId("bin-row-frida-server")) as Element);
    await screen.findByTestId("hosted-frida-server");
    expect(screen.queryByTestId("pid-frida-server")).toBeNull();
    expect(screen.queryByTestId("running-untracked-frida-server")).toBeNull();
    // 读不到运行表就照实说"未运行"：这一行没有设备侧凭据可显示。
    // 用终端起的那个进程不在设备运行表里，它的状态由右边那块屏自己说。
    expect(screen.getByTestId("hosted-frida-server").textContent ?? "").toContain("未运行");
    // 没凭据就不给「终止」，给的是启动入口
    expect(screen.queryByTestId("kill-frida-server")).toBeNull();
    expect(screen.getByTestId("hosted-terminal-frida-server")).toBeEnabled();
  });

  it("进程端口两个方向都能渲染（PID→端口、端口→PID）", async () => {
    renderPage(<ProcPorts />);
    // 两个 section 各有一个「查询」，DOM 顺序是「端口 → 进程」在前、「进程 → 端口」在后；
    // 这里不靠顺序猜数据：分别断言各自由哪个接口喂出来
    const pidBox = await screen.findByLabelText("进程 PID");
    fireEvent.change(pidBox, { target: { value: "25265" } });
    fireEvent.click((await screen.findAllByText("查询"))[1]);
    await waitFor(() => expect(device.procPorts).toHaveBeenCalled());
    const portBox = await screen.findByLabelText("端口");
    fireEvent.change(portBox, { target: { value: "27042" } });
    fireEvent.click((await screen.findAllByText("查询"))[0]);
    await waitFor(() => expect(device.procByPort).toHaveBeenCalled());
    expect(await screen.findByText("frida-server")).toBeTruthy();
  });
});

describe("任务中心", () => {
  it("任务类型与退出码来自 typed 字段，不靠解析可读名字", async () => {
    renderPage(<TasksPage />);
    await waitFor(() => expect(task.list).toHaveBeenCalled());
    expect(await screen.findByText("shell: getprop ro.build.version.sdk")).toBeTruthy();
    const failed = await screen.findByText("install base.apk");
    fireEvent.click(failed);
    // 详情里的退出码来自 typed 字段（不是解析任务名得来的）
    await waitFor(() => expect(document.body.textContent).toContain("退出码"));
    expect(document.body.textContent).toMatch(/1/);
  });
});

describe("设备页 Agent · 结论条在前，开发者诊断在后（第七十轮口径）", () => {
  const base = {
    status: {
      serial: "PIXEL-1",
      state: "ready",
      agentVersion: "0.2.1",
      protocolVersion: 1,
      artifactSha256: "aa",
      capabilities: [
        { method: "package.list", version: 1, provider: "shell", available: true, probePending: false },
      ],
      providers: [
        { name: "shell", version: "0.2.1", health: "ready", requiredPermissions: ["shell"], lastError: null },
      ],
      lastError: null,
    },
    health: null,
    healthError: null,
    routes: [],
    // 自动连接的探测结论（AR12.5）：null = 还没探过，结论句会退回"这台还没探"
    autoProbe: null,
  };

  /** AR12.1 那份删除证据：一条都不能少，只是不再摆在正文 */
  const FALLBACKS = [
    { method: "package.list", reason: "agent_unavailable", count: 3, removalStage: "AR12.1 after AR5.5" },
    { method: "device.info", reason: "unsupported_method", count: 1, removalStage: "AR12.1 after AR5.2" },
  ];

  const probeOf = (patch: Record<string, unknown>) => ({
    serial: "PIXEL-1",
    decision: "not_installed",
    action: "ask_consent",
    deviceAbi: "arm64-v8a",
    installedSha256: null,
    expectedSha256: "aa",
    agentRunning: false,
    consentGranted: false,
    autoEnabled: true,
    detail: null,
    probedAt: 1,
    ...patch,
  });

  it("计数为 0 时明确写\"全部走 Agent\"（这才是可删回退的证据）", async () => {
    agentMocks.diagnostics.mockResolvedValue({ ...base, legacyFallbacks: [] });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    const row = await screen.findByTestId("agent-legacy-fallbacks");
    expect(row.textContent).toContain("无（全部走 Agent）");
  });

  it("走过回退时按能力与原因分别显示次数", async () => {
    agentMocks.diagnostics.mockResolvedValue({ ...base, legacyFallbacks: FALLBACKS });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    const row = await screen.findByTestId("agent-legacy-fallbacks");
    expect(row.textContent).toContain("package.list × 3（agent_unavailable）");
    expect(row.textContent).toContain("device.info × 1（unsupported_method）");
  });

  /**
   * 用户那句"这些我看不懂、也不该我读"的机械化：回退计数与最近路由必须待在
   * `agent-dev-diagnostics` 这个折叠区**里面**。
   * 断言写成"在 details 内"而不是"看不见"：AR12.1 的观察点必须留着，
   * 挪走的是位置，不是数据。
   */
  it("回退计数与最近路由都在开发者诊断折叠区内", async () => {
    agentMocks.diagnostics.mockResolvedValue({
      ...base,
      legacyFallbacks: FALLBACKS,
      routes: [
        {
          serial: "PIXEL-1",
          method: "hosted.list",
          backend: "legacy_adb",
          fallbackReason: "agent_unavailable",
          agentVersion: "0.2.1",
          protocolVersion: 1,
          recordedAt: 1,
        },
      ],
    });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    const details = await screen.findByTestId("agent-dev-diagnostics");
    expect(details.tagName).toBe("DETAILS");
    expect(within(details).getByTestId("agent-legacy-fallbacks").textContent).toContain(
      "package.list × 3",
    );
    expect(within(details).getByText("hosted.list")).toBeTruthy();
    expect(within(details).getByText("Legacy ADB (agent_unavailable)")).toBeTruthy();
  });

  it("未连接且未授权：说清挡住了什么、只问一次，并把主按钮给出来", async () => {
    agentMocks.diagnostics.mockResolvedValue({
      ...base,
      status: { ...base.status, state: "adb_online" },
      legacyFallbacks: FALLBACKS,
      autoProbe: probeOf({}),
    });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    // 等的是那句只有数据到手才会出现的结论，不是等外壳（外壳在查询回来前就在了）
    await screen.findByText("Agent 还没连上。");
    const banner = screen.getByTestId("agent-purpose");
    // 挡住的是功能名，不是"capability 数"或"回退次数"
    expect(banner.textContent).toContain("权限、托管进程、终止进程、ROOT、SO 替换");
    expect(banner.textContent).toContain("这台只问一次");
    /*
      第七十三轮「结论条要更短」的机器化：正文只许有两句——一句结论 + 一句
      "挡住什么/代价"（授权说明并进同一行）。以前这里是三到四段小字堆着，
      用户抱怨"读不出重点"。行数一旦反弹，这条就会红。
    */
    expect(banner.querySelectorAll("p")).toHaveLength(2);
    fireEvent.click(within(banner).getByTestId("agent-connect-now"));
    await waitFor(() => expect(agentMocks.install).toHaveBeenCalledWith("PIXEL-1"));
  });

  it("别人的 Agent 在跑：给「重启并接管」，并把代价写在按钮旁边", async () => {
    agentMocks.diagnostics.mockResolvedValue({
      ...base,
      status: { ...base.status, state: "adb_online" },
      autoProbe: probeOf({
        decision: "running_elsewhere",
        action: "explicit_takeover",
        installedSha256: "aa",
        agentRunning: true,
        consentGranted: true,
        detail: "设备上已有 Agent 在跑，但不是本程序连上的",
      }),
    });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    await screen.findByText("有个 Agent 在跑，但不是本程序连的。");
    const banner = screen.getByTestId("agent-purpose");
    expect(banner.textContent).toContain("托管进程与 frida-server 会断");
    // 结论与原始原因不再各写一遍（同一件事在卡片里出现两次就是"长"）：
    // 正文只留结论，raw detail 进开发者诊断，数据没丢、但不再占正文。
    expect(banner.textContent).not.toContain("设备上已有 Agent 在跑，但不是本程序连上的");
    expect(screen.getByTestId("agent-dev-reason").textContent).toBe(
      "设备上已有 Agent 在跑，但不是本程序连上的",
    );
    expect(banner.querySelectorAll("p")).toHaveLength(2);
    fireEvent.click(within(banner).getByTestId("agent-takeover"));
    await waitFor(() => expect(agentMocks.restart).toHaveBeenCalledWith("PIXEL-1"));
    expect(agentMocks.install).not.toHaveBeenCalled();
  });

  /**
   * 装不了就别给"安装"按钮。这条钉的是上一版的自己：`blocked` 时也摆着
   * 「安装并连接」，点下去转一圈 adb、再把同一条原因还给她。
   */
  it("装不了（ABI 不支持）：只给原因与重新探测，不给注定失败的按钮", async () => {
    agentMocks.diagnostics.mockResolvedValue({
      ...base,
      status: { ...base.status, state: "disconnected" },
      autoProbe: probeOf({
        decision: "unsupported_abi",
        action: "blocked",
        deviceAbi: "armeabi-v7a",
        detail: "本机 Agent 产物只提供 arm64-v8a，这台设备是 armeabi-v7a",
      }),
    });
    agentMocks.probe.mockResolvedValue(probeOf({}));
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    await screen.findByText("本机 Agent 产物只提供 arm64-v8a，这台设备是 armeabi-v7a");
    const banner = screen.getByTestId("agent-purpose");
    // blocked 是唯一的例外：那句"为什么装不了"就是内容本身，必须留在正文
    expect(banner.textContent).toContain("armeabi-v7a");
    expect(within(banner).queryByTestId("agent-connect-now")).toBeNull();
    expect(within(banner).queryByTestId("agent-takeover")).toBeNull();
    fireEvent.click(within(banner).getByTestId("agent-reprobe"));
    await waitFor(() => expect(agentMocks.probe).toHaveBeenCalledWith("PIXEL-1"));
  });

  it("Agent 就绪时不弹结论条，正文只剩能力数", async () => {
    agentMocks.diagnostics.mockResolvedValue({ ...base, legacyFallbacks: [] });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    await screen.findByTestId("agent-legacy-fallbacks");
    expect(screen.queryByTestId("agent-purpose")).toBeNull();
    expect(screen.getByTestId("agent-status-PIXEL-1").textContent).toContain("就绪");
    // 就绪 = 没什么要用户决定，结论条不该占地方
    expect(screen.queryByTestId("agent-connect-now")).toBeNull();
  });
});

  /**
   * AR7.5：文件页的写侧接线。这里要看的不是"能不能编译"，而是**点一下之后
   * 到底调了哪条能力、传了什么路径**——路径拼错一个斜杠，界面就会报一句
   * 看起来像设备坏了的错（/sdcard/sdcard 那次就是这样）。
   */
  it("文件页：新建目录把拼好的完整路径交给 filesystem.mkdir", async () => {
    device.ls.mockResolvedValue(lsOf([{ name: "Download", isDir: true, size: 0, symlink: null, perms: "drwxrwx--x" }]));
    device.fsMkdir.mockResolvedValue({
      path: "/sdcard/新目录",
      created: true,
      mode: 0o777,
      mode_text: "drwxrwxrwx",
    });
    renderPage(<FilesView serial="PIXEL-1" />);
    fireEvent.click(await screen.findByText("新建目录"));
    const box = await screen.findByRole("textbox", { name: "新建目录" });
    fireEvent.change(box, { target: { value: "新目录" } });
    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(device.fsMkdir).toHaveBeenCalled());
    // 第三位是这台设备的 root 意图（探得过 su 就默认 true），路径拼法仍是主角
    expect(device.fsMkdir).toHaveBeenCalledWith("PIXEL-1", "/sdcard/新目录", expect.any(Boolean));
    // 结论要说"建了还是本来就在"，不能只说成功
    await waitFor(() => expect(document.body.textContent).toContain("目录已就绪"));
  });

  it("文件页：Root (su) 默认开着，增删改按 root 身份发给设备", async () => {
    device.ls.mockResolvedValue(lsOf([]))
    device.binarySuCheck.mockResolvedValue(true);
    device.fsMkdir.mockResolvedValue({
      path: "/sdcard/x",
      created: true,
      mode: 0o777,
      mode_text: "drwxrwxrwx",
    });
    renderPage(<FilesView serial="PIXEL-1" />);
    const box = (await screen.findByRole("checkbox", { name: "Root (su)" })) as HTMLInputElement;
    // 用户要的"默认给 su 权限"：探得过就开，不用他每件事手动勾一次
    await waitFor(() => expect(box.checked).toBe(true));
    // 范围也摆在台面上：给的是权限，不是放开整个文件系统
    expect((await screen.findByTestId("fs-write-scope")).textContent ?? "").toContain(
      "/data/local/tmp",
    );
    fireEvent.click(screen.getByRole("button", { name: "新建目录" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "新建目录" }), {
      target: { value: "x" },
    });
    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(device.fsMkdir).toHaveBeenCalledWith("PIXEL-1", "/sdcard/x", true));
  });

  it("文件页：探不到 su 就按 shell 执行并说破，不假装 root 已生效", async () => {
    device.ls.mockResolvedValue(lsOf([]))
    device.binarySuCheck.mockResolvedValue(false);
    device.fsMkdir.mockResolvedValue({
      path: "/sdcard/x",
      created: true,
      mode: 0o777,
      mode_text: "drwxrwxrwx",
    });
    renderPage(<FilesView serial="PIXEL-1" />);
    const box = (await screen.findByRole("checkbox", { name: "Root (su)" })) as HTMLInputElement;
    await waitFor(() => expect(box.checked).toBe(false));
    await screen.findByTestId("fs-root-notice");
    fireEvent.click(screen.getByRole("button", { name: "新建目录" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "新建目录" }), {
      target: { value: "x" },
    });
    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(device.fsMkdir).toHaveBeenCalledWith("PIXEL-1", "/sdcard/x", false));
  });

  it("文件页：`sdcard` 这种链接点名字就能进去，也不再冒「目录没有预览」", async () => {
    // 用户就是在这儿撞见的：根目录下点 sdcard ⇒ 一栏 symlink 元数据 + 一句"预览失败：目录没有预览"。
    // 根因两处：元数据按 lstat 读（链接自己），预览又无条件对每一行发起。
    device.binarySuCheck.mockResolvedValue(false);
    device.ls.mockResolvedValue(
      lsOf(
        [{ name: "sdcard", isDir: false, size: 21, symlink: "/storage/emulated/0", perms: "lrwxrwxrwx" }],
        { path: "/" },
      ),
    );
    device.fileStat.mockResolvedValue({
      requested_path: "/sdcard",
      path: "/storage/emulated/0",
      stat: {
        name: "sdcard",
        kind: "dir",
        mode: 0o2777,
        mode_text: "drwxrwsrwx",
        uid: 1023,
        gid: 1023,
        size: 3452,
        mtime_unix: 1_230_768_000,
        symlink_target: null,
        readable: true,
      },
    });
    device.filePreview.mockRejectedValue(new Error("不该被调用"));
    renderPage(<FilesView serial="PIXEL-1" />);
    // 这一条测的就是"在根目录点 sdcard"：不先退到 /，行路径会拼成 /sdcard/sdcard（那是另一件事）
    fireEvent.click(await screen.findByRole("button", { name: "上一级" }));
    const row = (await screen.findByTestId("fs-info-sdcard")).closest("li")!;
    // 先看它本身（行内 ⓘ）：元数据跟链读 ⇒ kind=dir ⇒ 这一行根本不该发预览请求
    fireEvent.click(within(row).getByTestId("fs-info-sdcard"));
    await waitFor(() =>
      expect(device.fileStat).toHaveBeenCalledWith("PIXEL-1", "/sdcard", true, false),
    );
    // 也就没有那句红字
    expect(device.filePreview).not.toHaveBeenCalled();
    expect(screen.queryByText(/预览失败/)).toBeNull();
    // 名字与真实路径之间要有分隔：没有它就成了 "sdcard" + "/storage/emulated/0" 糊成一条路径
    const head = (await screen.findByTestId("files-stat-path")).parentElement?.textContent ?? "";
    expect(head).toContain("·");
    // 点链接的名字就是**跟进去**：旧规则按列表那一行的 lstat 类型决定能不能点，
    // symlink 没有尾斜杠 ⇒ 永远只能被选中，正是用户报的"末尾没有 / 就点不进去"
    fireEvent.click(within(row).getByText("sdcard"));
    await waitFor(() => expect(device.ls).toHaveBeenLastCalledWith("PIXEL-1", "/sdcard", false));
  });

  it("文件页：读不到就说读不到，绝不画成「这个目录是空的」", async () => {
    // 用户报的"/data 下面是空的"就是这么来的：Agent 以 shell 身份 opendir 失败，
    // 设备回的是「条目为空 + 一条读不到的原因」，旧接口把原因丢了 ⇒ 界面只能说"空目录"。
    device.binarySuCheck.mockResolvedValue(false);
    device.ls.mockResolvedValue(
      lsOf([], { path: "/data", unreadable: ["/data: Permission denied (13)"] }),
    );
    renderPage(<FilesView serial="PIXEL-1" />);
    const why = await screen.findByText(/读不到/);
    expect(why.textContent ?? "").toContain("Permission denied");
    // 还要给一句能照着做的下一步
    expect(why.textContent ?? "").toContain("Root (su)");
    expect(why.textContent ?? "").not.toContain("这个目录是空的");
  });

  it("文件页：开着 Root 时列目录也按 root 身份问设备，并写明本屏读自谁", async () => {
    device.binarySuCheck.mockResolvedValue(true);
    device.ls.mockResolvedValue(lsOf([], { path: "/sdcard", source: "root" }));
    renderPage(<FilesView serial="PIXEL-1" />);
    await waitFor(() => expect(device.ls).toHaveBeenCalledWith("PIXEL-1", "/sdcard", true));
    await waitFor(() =>
      expect(screen.getByTestId("fs-read-source").textContent ?? "").toContain("root"),
    );
  });

  it("文件页：出了那两棵树就不给改，按钮禁着并说清只能在哪儿", async () => {
    device.ls.mockResolvedValue(lsOf([]))
    renderPage(<FilesView serial="PIXEL-1" />);
    fireEvent.click(await screen.findByRole("button", { name: "上一级" }));
    const mkdir = (await screen.findByRole("button", {
      name: "新建目录",
    })) as HTMLButtonElement;
    await waitFor(() => expect(mkdir.disabled).toBe(true));
    // 被禁必须给理由，而且理由里要有"那我能在哪儿改"
    expect(mkdir.title).toContain("只能在这两处之内");
    expect(mkdir.title).toContain("/sdcard");
    expect(screen.getByRole("button", { name: /上传/ }).hasAttribute("disabled")).toBe(true);
  });

  it("文件页：删除必须先确认，且目录的递归要显式勾选", async () => {
    device.ls.mockResolvedValue(lsOf([
      { name: "old.log", isDir: false, size: 12, symlink: null, perms: "-rw-r--r--" },
      { name: "stuff", isDir: true, size: 0, symlink: null, perms: "drwxr-xr-x" },
    ]));
    device.fsRemove.mockResolvedValue({
      path: "/sdcard/old.log",
      removed: true,
      was_dir: false,
      was_recursive: false,
      freed_bytes: 12,
    });
    renderPage(<FilesView serial="PIXEL-1" />);
    const row = (await screen.findByText("old.log")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: "删除" }));
    // 一次点击只该进入确认态，不能直接就把文件删了
    expect(device.fsRemove).not.toHaveBeenCalled();
    await screen.findByText(/确认删除/);
    fireEvent.click(screen.getByTestId("fs-remove-confirm"));
    await waitFor(() =>
      expect(device.fsRemove).toHaveBeenCalledWith("PIXEL-1", "/sdcard/old.log", false, expect.any(Boolean)),
    );
    await waitFor(() => expect(document.body.textContent).toContain("已删除 /sdcard/old.log"));

    // 目录：确认条上必须出现"连内容一起删"这条显式选择
    // 目录行渲染成 "stuff/"（斜杠是界面给的视觉提示），断言别被它骗到
    const dirRow = screen.getByText(/^stuff\/$/).closest("li")!;
    fireEvent.click(within(dirRow).getByRole("button", { name: "删除" }));
    await waitFor(() =>
      expect(screen.getByLabelText(/连目录内容一起删/)).toBeTruthy(),
    );
  });

  /**
   * 钉住"动作必须看得见"这件事。上一版把行内动作写成 `opacity-0` + 悬停显示，
   * jsdom 不解析 CSS，所以三条接线测试全绿，而用户在界面上找不到删除与取回入口
   * ——测试通过不等于功能存在。类名检查能挡住"再用 opacity-0 藏起来"这种回退。
   */
  it("文件页：删除与取回入口默认可见，不靠悬停才出现", async () => {
    device.ls.mockResolvedValue(lsOf([
      { name: "tool", isDir: false, size: 5, symlink: null, perms: "-rw-r--r--" },
      { name: "stuff", isDir: true, size: 0, symlink: null, perms: "drwxr-xr-x" },
    ]));
    renderPage(<FilesView serial="PIXEL-1" />);
    const fileRow = (await screen.findByText("tool")).closest("li")!;
    const actions = within(fileRow).getByTestId("fs-row-actions");
    expect(actions.className).not.toContain("opacity-0");
    // 每一行都要能看见四个动作入口；目录额外没有"改权限以外"的缺失
    expect(
      within(fileRow)
        .getAllByRole("button")
        .map((b) => b.getAttribute("aria-label"))
        .filter(Boolean),
    ).toEqual(["取回本地…", "重命名", "改权限", "删除"]);
    const dirRow = screen.getByText(/^stuff\/$/).closest("li")!;
    // 能点进去的行（目录、链接）多一个"只看它本身"的入口：跟错链接时不用退回上级去查它
    expect(
      within(dirRow)
        .getAllByRole("button")
        .map((b) => b.getAttribute("aria-label"))
        .filter(Boolean),
    ).toEqual(["只看它本身的元数据", "取回本地…", "重命名", "改权限", "删除"]);
  });

  it("文件页：改权限把八进制按数值送出去，并把设备回读的实际值显示出来", async () => {
    device.ls.mockResolvedValue(lsOf([{ name: "tool", isDir: false, size: 5, symlink: null, perms: "-rw-r--r--" }]));
    device.fsChmod.mockResolvedValue({
      path: "/sdcard/tool",
      mode: 0o700,
      mode_text: "-rwx------",
      previous_mode: 0o644,
      previous_mode_text: "-rw-r--r--",
      verified: false,
    });
    renderPage(<FilesView serial="PIXEL-1" />);
    const row = (await screen.findByText("tool")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: "改权限" }));
    const box = await screen.findByPlaceholderText(/八进制|Octal/);
    fireEvent.change(box, { target: { value: "700" } });
    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(device.fsChmod).toHaveBeenCalledWith("PIXEL-1", "/sdcard/tool", 0o700, expect.any(Boolean)));
    // verified=false 要说"实际生效"，不能照抄请求值假装成功
    await waitFor(() => expect(document.body.textContent).toContain("设备实际生效 -rwx------"));
  });
