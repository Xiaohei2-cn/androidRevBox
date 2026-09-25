import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  shell: vi.fn(),
  logcat: vi.fn(),
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
  useActiveTab: () => true,
}));
vi.mock("@/hooks/useDragDropPath", () => ({ useDragDropPath: vi.fn() }));
const agentMocks = vi.hoisted(() => ({ diagnostics: vi.fn(), install: vi.fn(), restart: vi.fn() }));
vi.mock("@/api/agent", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/agent")>();
  return { ...actual, agentApi: { ...actual.agentApi, ...agentMocks } };
});

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
  device.environment.mockResolvedValue({ installed: true, path: "/sdk/adb", source: "path_env" });
  device.list.mockResolvedValue([
    { serial: "PIXEL-1", state: "device", transport: "usb", model: "Pixel 6" },
  ]);
  device.onChanged.mockResolvedValue(() => {});
  device.rootCheck.mockResolvedValue(true);
  device.binarySuCheck.mockResolvedValue(true);
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
  device.binaries.mockResolvedValue([
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
  ]);
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

  it("Shell 与 Logcat 明确标成 ADB 原始会话，且各自打到自己的 command", async () => {
    device.shell.mockResolvedValue("task-1");
    device.logcat.mockResolvedValue("task-2");
    renderPage(<DevicesPage />);
    fireEvent.click(await screen.findByRole("tab", { name: "Shell" }));
    // 边界文案必须出现在界面上（AR9.3 的分类不能只写在文档里）
    expect(document.body.textContent).toContain("不经 Agent");
    const box = await screen.findByPlaceholderText(/input text hello/);
    fireEvent.change(box, { target: { value: "getprop ro.product.model" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(device.shell).toHaveBeenCalledWith("PIXEL-1", "getprop ro.product.model"));

    fireEvent.click(screen.getByRole("tab", { name: "Logcat" }));
    const filter = await screen.findByPlaceholderText(/如 crash/);
    fireEvent.change(filter, { target: { value: "ActivityManager" } });
    fireEvent.keyDown(filter, { key: "Enter" });
    await waitFor(() => expect(device.logcat).toHaveBeenCalledWith("PIXEL-1", "ActivityManager"));
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
    // 本地模型（camelCase）：权限串决定绿/红与能否双击入区
    expect(await screen.findByText("-rwxr-xr-x")).toBeTruthy();
    // 协议 DTO（snake_case：start_time_ticks / started_at_unix / handle / state）
    // 决定托管区里这一行是不是「在跑、pid 多少、能不能按句柄停止」。
    // 曾经这里读的是 startedAtUnix → undefined，行就永远显示未运行。
    await waitFor(() => expect(device.hostedRuns).toHaveBeenCalled());
    expect(await screen.findByText(/25265/)).toBeTruthy();
    expect(await screen.findByText("终止")).toBeTruthy();
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
    fireEvent.doubleClick((screen.getByTestId("bin-frida-server").querySelector("button") ??
      screen.getByTestId("bin-frida-server")) as Element);
    const hosted = await screen.findByTestId("hosted-frida-server");
    expect(hosted.textContent ?? "").toContain("已在运行 pid 31337");
    // 关键反向断言：这一行**没有**「执行」按钮 —— 点了也只会起一个秒退的进程
    expect(screen.queryByTestId("run-frida-server")).toBeNull();
    device.binaryRun.mockClear();
    device.binaryKill.mockClear();
    // 停止后清单会重新拉一次：这次报告"没有外部实例了"，模拟进程真被停掉
    device.binaries.mockResolvedValue([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]);
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
    // 停下来之后，同一行才变回「执行」
    await waitFor(() => expect(screen.getByTestId("run-frida-server")).toBeTruthy());
  });

  it("就算界面状态是旧的，桌面侧拦下了第二次启动也只说「已经在跑」，不报成失败", async () => {
    // binaryRun 返回 started=false：这是"没起新的"，不是"起失败了"——
    // 报红色失败会让人以为自己的二进制坏了。
    device.hostedRuns.mockResolvedValue([]);
    device.binaries.mockResolvedValue([
      {
        name: "frida-server",
        path: "/data/local/tmp/frida-server",
        size: 53_539_200,
        perms: "-rwxr-xr-x",
        hasExec: true,
        externalProcs: [],
      },
    ]);
    device.binaryRun.mockResolvedValue({
      pid: 4242,
      started: false,
      detail: "frida-server 已经在跑（pid 4242 · 父进程 3900 · uid 2000），不在本工具的托管表里",
    });
    renderPage(<BinaryHosting />);
    // 清单是异步来的，先等它出现（上一条测试是"先等到有外部实例"才动的，这里同理）
    const row = await screen.findByTestId("bin-frida-server");
    fireEvent.doubleClick((row.querySelector("button") ?? row) as Element);
    await screen.findByTestId("hosted-frida-server");
    fireEvent.click(screen.getByTestId("run-frida-server"));
    await waitFor(() =>
      expect(screen.getAllByText(/已经在跑|已有一个实例在跑/).length).toBeGreaterThan(0),
    );
    expect(document.body.textContent ?? "").not.toContain("启动失败");
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

describe("设备页 Agent 诊断 · Legacy 回退计数（AR12 的删除依据）", () => {
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
  };

  it("计数为 0 时明确写\"全部走 Agent\"（这才是可删回退的证据）", async () => {
    agentMocks.diagnostics.mockResolvedValue({ ...base, legacyFallbacks: [] });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    const row = await screen.findByTestId("agent-legacy-fallbacks");
    expect(row.textContent).toContain("无（全部走 Agent）");
  });

  it("走过回退时按能力与原因分别显示次数", async () => {
    agentMocks.diagnostics.mockResolvedValue({
      ...base,
      legacyFallbacks: [
        {
          method: "package.list",
          reason: "agent_unavailable",
          count: 3,
          removalStage: "AR12.1 after AR5.5",
        },
        {
          method: "device.info",
          reason: "unsupported_method",
          count: 1,
          removalStage: "AR12.1 after AR5.2",
        },
      ],
    });
    renderPage(<AgentSessionSection serial="PIXEL-1" />);
    const row = await screen.findByTestId("agent-legacy-fallbacks");
    expect(row.textContent).toContain("package.list × 3（agent_unavailable）");
    expect(row.textContent).toContain("device.info × 1（unsupported_method）");
  });
});

  /**
   * AR7.5：文件页的写侧接线。这里要看的不是"能不能编译"，而是**点一下之后
   * 到底调了哪条能力、传了什么路径**——路径拼错一个斜杠，界面就会报一句
   * 看起来像设备坏了的错（/sdcard/sdcard 那次就是这样）。
   */
  it("文件页：新建目录把拼好的完整路径交给 filesystem.mkdir", async () => {
    device.ls.mockResolvedValue([{ name: "Download", isDir: true, size: 0, symlink: null, perms: "drwxrwx--x" }]);
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
    expect(device.fsMkdir).toHaveBeenCalledWith("PIXEL-1", "/sdcard/新目录");
    // 结论要说"建了还是本来就在"，不能只说成功
    await waitFor(() => expect(document.body.textContent).toContain("目录已就绪"));
  });

  it("文件页：删除必须先确认，且目录的递归要显式勾选", async () => {
    device.ls.mockResolvedValue([
      { name: "old.log", isDir: false, size: 12, symlink: null, perms: "-rw-r--r--" },
      { name: "stuff", isDir: true, size: 0, symlink: null, perms: "drwxr-xr-x" },
    ]);
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
    await waitFor(() => expect(device.fsRemove).toHaveBeenCalledWith("PIXEL-1", "/sdcard/old.log", false));
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
    device.ls.mockResolvedValue([
      { name: "tool", isDir: false, size: 5, symlink: null, perms: "-rw-r--r--" },
      { name: "stuff", isDir: true, size: 0, symlink: null, perms: "drwxr-xr-x" },
    ]);
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
    expect(
      within(dirRow)
        .getAllByRole("button")
        .map((b) => b.getAttribute("aria-label"))
        .filter(Boolean),
    ).toEqual(["取回本地…", "重命名", "改权限", "删除"]);
  });

  it("文件页：改权限把八进制按数值送出去，并把设备回读的实际值显示出来", async () => {
    device.ls.mockResolvedValue([{ name: "tool", isDir: false, size: 5, symlink: null, perms: "-rw-r--r--" }]);
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
    await waitFor(() => expect(device.fsChmod).toHaveBeenCalledWith("PIXEL-1", "/sdcard/tool", 0o700));
    // verified=false 要说"实际生效"，不能照抄请求值假装成功
    await waitFor(() => expect(document.body.textContent).toContain("设备实际生效 -rwx------"));
  });
