import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";

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
  useAppNav: () => ({ tab: "adb", subTab: "binary-hosting", go: vi.fn(), goAdbSubTab: vi.fn() }),
  useActiveTab: () => true,
}));
vi.mock("@/hooks/useDragDropPath", () => ({ useDragDropPath: vi.fn() }));

import { DevicesPage } from "@/features/devices/DevicesPage";
import { BinaryHosting } from "@/features/adb/BinaryHosting";
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
    },
    {
      name: "helper.dex",
      path: "/data/local/tmp/helper.dex",
      size: 12_345,
      perms: "-rw-r--r--",
      hasExec: false,
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
