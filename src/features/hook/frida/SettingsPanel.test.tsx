/**
 * A 区那两个分段选择（连接 / 运行模式）的选中态（第九十三轮用户要求：选中＝蓝）。
 *
 * 之前选中的那颗是 `bg-background`（浅底 + 影子），在深色主题下几乎看不出"哪个是当前值"；
 * 现在走主色 `bg-primary text-primary-foreground`——就是设置页分段选择、左侧 tab 齿块、
 * 「启动」按钮共用的那一个蓝，不再新造一种"选中"的语言。
 *
 * 判据刻意落在语义（aria-checked）+ 类名（主色只挂选中的那颗）上：
 * jsdom 没有排版，量不到像素，能钉的是"我们交给样式的是哪一套语言"。
 */
import { readFileSync } from "node:fs";
import { beforeEach } from "vitest";
import { useState } from "react";
import path from "node:path";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";
import { SettingsPanel } from "./SettingsPanel";
import type { FridaSettings } from "./types";
import { describe, expect, it, vi } from "vitest";

const join = path.join;

import { Segmented } from "./SettingsPanel";

const OPTIONS = [
  { v: "usb", label: "USB" },
  { v: "remote", label: "远程端口" },
];

function paintOf(name: string) {
  return screen.getByRole("radio", { name }).className;
}

describe("A 区分段选择的选中态", () => {
  it("选中的那颗是主色，另一颗不是", () => {
    render(<Segmented value="usb" options={OPTIONS} onChange={vi.fn()} />);
    expect(paintOf("USB")).toContain("bg-primary");
    expect(paintOf("USB")).toContain("text-primary-foreground");
    expect(paintOf("USB")).not.toContain("bg-background");
    expect(paintOf("远程端口")).not.toContain("bg-primary");
    expect(screen.getByRole("radio", { name: "USB" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "远程端口" })).toHaveAttribute("aria-checked", "false");
  });

  it("换值之后主色跟着走（不是写死在第一颗上）", () => {
    const onChange = vi.fn();
    const { rerender } = render(<Segmented value="usb" options={OPTIONS} onChange={onChange} />);
    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    expect(onChange).toHaveBeenCalledWith("remote");

    rerender(<Segmented value="remote" options={OPTIONS} onChange={onChange} />);
    expect(paintOf("远程端口")).toContain("bg-primary");
    expect(paintOf("USB")).not.toContain("bg-primary");
  });

  it("会话跑起来了就整组禁用：选中样式照旧，点不动", () => {
    const onChange = vi.fn();
    render(<Segmented value="usb" options={OPTIONS} onChange={onChange} disabled />);
    expect(paintOf("USB")).toContain("bg-primary");
    for (const name of ["USB", "远程端口"]) {
      expect(screen.getByRole("radio", { name })).toBeDisabled();
    }
    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    expect(onChange).not.toHaveBeenCalled();
  });
});
/**
 * 设备侧 frida-server 的启停**不属于**会话设置面板。
 *
 * 以前这里有「以 root 启动」「停止」两个按钮：同一件事两个入口，而且这个面板
 * 不知道起的是哪个二进制、什么版本、日志在哪——那些全在「二进制」页的托管列表里。
 * 现在这里只读状态 + 一个跳过去的箭头，所以这里断言的是"别再把起停写回来"。
 */
describe("A 区不管 frida-server 的起停", () => {
  const src = readFileSync(
    join(process.cwd(), "src/features/hook/frida/SettingsPanel.tsx"),
    "utf8",
  );

  it("不调用启动/停止接口", () => {
    expect(src).not.toContain("fridaServerStart");
    expect(src).not.toContain("fridaServerStop");
  });

  it("改成跳到二进制托管页（唯一入口在那边）", () => {
    expect(src).toContain("gotoBinarySubTab");
    expect(src).toContain("frida-server-goto-hosting");
  });
});

/**
 * 会话设置的连接目标行与「强行停止」。
 *
 * 两件要钉住的：
 * 1. 切 USB / 远程端口**不许改变布局** —— 承载变化的那一行必须是同一个 DOM 节点，
 *    变的只是里面的内容（以前端口框塞在第一行里，一点整行横向跳一格）。
 * 2. 强停走的是设备侧 typed 命令（`device_force_stop`，带复核），
 *    没有包名或填的是 pid 时不给点 —— 那是个杀进程的动作，不能让人误触。
 */
const api = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
  fridaServerStatus: vi.fn(),
  forceStop: vi.fn(),
}));
const hook = vi.hoisted(() => ({ preflight: vi.fn() }));
const env = vi.hoisted(() => ({ foreground: vi.fn() }));

vi.mock("@/api/device", () => ({
  deviceApi: {
    environment: api.environment,
    list: api.list,
    onChanged: api.onChanged,
    fridaServerStatus: api.fridaServerStatus,
    forceStop: api.forceStop,
  },
}));
vi.mock("@/api/hook", () => ({ hookApi: { preflight: hook.preflight } }));
vi.mock("@/api/env", () => ({ envApi: { foreground: env.foreground } }));

/** 受控组件得有状态才测得出"切换"：直接给 no-op onChange 是假测试 */
function Harness({ initial }: { initial?: Partial<FridaSettings> }) {
  const [settings, setSettings] = useState<FridaSettings>({
    deviceSerial: "S1",
    connMode: "usb",
    port: "27042",
    runMode: "attach",
    target: "",
    ...initial,
  });
  return (
    <SettingsPanel
      settings={settings}
      onChange={(patch) => setSettings((prev) => ({ ...prev, ...patch }))}
    />
  );
}

function renderPanel(initial?: Partial<FridaSettings>) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nProvider>
        <Harness initial={initial} />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("连接目标行：切换不改变布局", () => {
  beforeEach(() => {
    api.environment.mockResolvedValue({ installed: true, path: "/adb", source: "path_env" });
    api.list.mockResolvedValue([{ serial: "S1", state: "device", transport: "usb", model: "Pixel" }]);
    api.onChanged.mockResolvedValue(() => undefined);
    api.fridaServerStatus.mockResolvedValue({
      state: "not_running",
      running: false,
      as_root: false,
      binary_name: "frida-server",
      listening: false,
    });
    hook.preflight.mockResolvedValue({
      adbOk: true,
      pythonOk: true,
      fridaOk: true,
      fridaVersion: "17.17.0",
      channelOk: null,
    });
  });

  it("两种模式下都在，而且是同一个节点（只换内容，不换结构）", async () => {
    renderPanel();
    const row = await screen.findByTestId("frida-endpoint-row");
    expect(await within(row).findByText("usb · S1")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    expect(screen.getByTestId("frida-endpoint-row")).toBe(row);
    // 端口输入框只出现在这一行里，第一行没被塞宽
    expect(within(row).getByTestId("frida-port")).toBeInTheDocument();
    expect(await within(row).findByText("127.0.0.1:27042")).toBeInTheDocument();
  });

  it("改端口，连接目标立刻跟着变（不用先点一下别的）", async () => {
    renderPanel();
    fireEvent.click(screen.getByRole("radio", { name: "远程端口" }));
    const row = screen.getByTestId("frida-endpoint-row");
    fireEvent.change(within(row).getByTestId("frida-port"), { target: { value: "31337" } });
    expect(await within(row).findByText("127.0.0.1:31337")).toBeInTheDocument();
  });
});

describe("强行停止目标应用（attach 现取前台，spawn 用框里填的包名）", () => {
  beforeEach(() => {
    api.environment.mockResolvedValue({ installed: true, path: "/adb", source: "path_env" });
    api.list.mockResolvedValue([{ serial: "S1", state: "device", transport: "usb", model: "Pixel" }]);
    api.onChanged.mockResolvedValue(() => undefined);
    api.fridaServerStatus.mockResolvedValue({
      state: "not_running",
      running: false,
      as_root: false,
      binary_name: "frida-server",
      listening: false,
    });
    hook.preflight.mockResolvedValue({
      adbOk: true,
      pythonOk: true,
      fridaOk: true,
      channelOk: null,
    });
    api.forceStop.mockReset();
    env.foreground.mockReset();
  });

  it("attach：目标输入框与「取前台」禁用，强停仍可点", async () => {
    renderPanel();
    const input = await screen.findByTestId("frida-target");
    expect(input).toBeDisabled();
    expect(input).toHaveAttribute("placeholder", "附加设备当前前台（-F）");
    // 禁用归禁用，值也不能显示出来：attach 模式下那个值根本不参与决定连谁
    expect((input as HTMLInputElement).value).toBe("");
    expect(screen.getByRole("button", { name: "取前台" })).toBeDisabled();
    expect(screen.getByTestId("frida-force-stop")).toBeEnabled();
  });

  it("attach：强停先向设备取一次前台包名，再打 force_stop", async () => {
    env.foreground.mockResolvedValue({ package: "com.front.app" });
    api.forceStop.mockResolvedValue({
      action: "force_stop",
      package: "com.front.app",
      operation_id: "op-1",
      outcome: "executed",
      verified: true,
      ran_as_root: true,
    });
    renderPanel();
    fireEvent.click(await screen.findByTestId("frida-force-stop"));
    await waitFor(() => expect(api.forceStop).toHaveBeenCalledWith("S1", "com.front.app"));
    expect(await screen.findByText("com.front.app 已强行停止（复核到进程消失）")).toBeInTheDocument();
  });

  it("取不到前台就什么都不做，并把原因说出来", async () => {
    env.foreground.mockResolvedValue({ package: null, error: "adb 不可用，前台应用检测暂停。" });
    renderPanel();
    fireEvent.click(await screen.findByTestId("frida-force-stop"));
    await waitFor(() => expect(env.foreground).toHaveBeenCalledWith("S1"));
    expect(api.forceStop).not.toHaveBeenCalled();
    expect(await screen.findByText(/强停作罢/)).toBeInTheDocument();
  });

  it("spawn：目标必须是包名；纯数字或空都不给点，有包名则不再去取前台", async () => {
    const spawnPid = renderPanel({ runMode: "spawn", target: "12345" });
    expect(await screen.findByTestId("frida-force-stop")).toBeDisabled();
    spawnPid.unmount();

    api.forceStop.mockResolvedValue({
      action: "force_stop",
      package: "com.demo.app",
      operation_id: "op-2",
      outcome: "executed",
      verified: false,
      detail: "pid 4321 还在",
      ran_as_root: true,
    });
    renderPanel({ runMode: "spawn", target: "com.demo.app" });
    fireEvent.click(await screen.findByTestId("frida-force-stop"));
    await waitFor(() => expect(api.forceStop).toHaveBeenCalledWith("S1", "com.demo.app"));
    expect(env.foreground).not.toHaveBeenCalled();
    // 复核没看到进程消失就不能写成成功
    expect(await screen.findByText(/pid 4321 还在/)).toBeInTheDocument();
  });
});

describe("frida-server 状态：认不出名字时不许谎报未运行", () => {
  beforeEach(() => {
    api.environment.mockResolvedValue({ installed: true, path: "/adb", source: "path_env" });
    api.list.mockResolvedValue([{ serial: "S1", state: "device", transport: "usb", model: "Pixel" }]);
    api.onChanged.mockResolvedValue(() => undefined);
    env.foreground.mockResolvedValue({ package: null });
    api.forceStop.mockResolvedValue({});
  });

  it("Agent 按进程名找不到（改名过的 server），但握手通了：说通道已通、身份不猜", async () => {
    api.fridaServerStatus.mockResolvedValue({
      state: "not_running",
      running: false,
      as_root: false,
      binary_name: "frida-server",
      listening: false,
    });
    hook.preflight.mockResolvedValue({
      adbOk: true,
      pythonOk: true,
      fridaOk: true,
      channelOk: true,
      channelHint: null,
    });
    renderPanel();
    expect(await screen.findByText(/端口 27042 已通/)).toBeInTheDocument();
    expect(screen.queryByText("未运行：远程模式会连不上；点右侧箭头去托管页启动")).toBeNull();
  });

  it("握手也没通才算未运行", async () => {
    api.fridaServerStatus.mockResolvedValue({
      state: "not_running",
      running: false,
      as_root: false,
      binary_name: "frida-server",
      listening: false,
    });
    hook.preflight.mockResolvedValue({
      adbOk: true,
      pythonOk: true,
      fridaOk: true,
      channelOk: false,
      channelHint: "连不上设备上的 frida-server",
    });
    renderPanel();
    expect(await screen.findByText("连不上设备上的 frida-server")).toBeInTheDocument();
  });
});
