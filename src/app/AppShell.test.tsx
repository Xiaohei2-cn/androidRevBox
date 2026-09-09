import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppShell } from "@/app/AppShell";

/**
 * P7 交互反馈回归：
 * 1) 卡片「去配置」→ 跳设置页，目标输入框聚焦 + config-flash 蓝框闪烁；
 * 2) keep-mounted：页面切换不卸载（查询状态保活），非激活页 hidden；
 * 3) PathText 双击全选。
 */

const mocks = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
  python: vi.fn(),
  node: vi.fn(),
  frida: vi.fn(),
  idaMcp: vi.fn(),
  jadxMcp: vi.fn(),
  foreground: vi.fn(),
  overview: vi.fn(),
  taskList: vi.fn(),
  snapshot: vi.fn(),
  set: vi.fn(),
  setLogLevel: vi.fn(),
  ping: vi.fn(),
}));

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return {
    ...actual,
    deviceApi: { ...actual.deviceApi, environment: mocks.environment, list: mocks.list, onChanged: mocks.onChanged },
  };
});

vi.mock("@/api/env", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/env")>();
  return {
    ...actual,
    envApi: {
      ...actual.envApi,
      python: mocks.python,
      node: mocks.node,
      frida: mocks.frida,
      idaMcp: mocks.idaMcp,
      jadxMcp: mocks.jadxMcp,
      foreground: mocks.foreground,
      overview: mocks.overview,
    },
  };
});

vi.mock("@/api/task", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/task")>();
  return { ...actual, taskApi: { ...actual.taskApi, list: mocks.taskList } };
});

vi.mock("@/api/config", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/config")>();
  return {
    ...actual,
    configApi: { ...actual.configApi, snapshot: mocks.snapshot, set: mocks.set, setLogLevel: mocks.setLogLevel },
  };
});

vi.mock("@/api/system", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/system")>();
  return { ...actual, systemApi: { ...actual.systemApi, ping: mocks.ping } };
});

function renderShell() {
  return render(
    <AppProviders>
      <AppShell />
    </AppProviders>,
  );
}

describe("AppShell（P7 交互）", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.environment.mockResolvedValue({ installed: false, hint: "无 adb" });
    mocks.list.mockResolvedValue([]);
    mocks.onChanged.mockResolvedValue(() => {});
    mocks.python.mockResolvedValue({ configured: false, ready: false, hint: "未配置" });
    mocks.node.mockResolvedValue({ ready: false, hint: "未检测到 node" });
    mocks.frida.mockResolvedValue({ pythonReady: false, installed: false });
    mocks.idaMcp.mockResolvedValue({ reachable: false, port: 13337 });
    mocks.jadxMcp.mockResolvedValue({ reachable: false, port: 8650 });
    mocks.foreground.mockResolvedValue({ state: "adb_unavailable", procPaths: [] });
    mocks.overview.mockResolvedValue({});
    mocks.taskList.mockResolvedValue([]);
    mocks.snapshot.mockResolvedValue([]);
    mocks.set.mockResolvedValue(null);
    mocks.setLogLevel.mockResolvedValue("info");
    mocks.ping.mockResolvedValue({ appVersion: "0.1.0", tauriVersion: "2.8.0", os: "macos", arch: "arm64" });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("keep-mounted：非激活页面在 DOM 中但 hidden（aria-hidden）", () => {
    renderShell();
    // 设置页的滑杆仍在 DOM（aria-hidden 子树，byRole 需显式 hidden:true）
    const slider = screen.getByRole("slider", { hidden: true });
    expect(slider).toBeInTheDocument();
    expect(slider.closest(".hidden")).not.toBeNull();
    // 仪表盘内容可见（无 .hidden 祖先）
    const grid = screen.getByTestId("dashboard-grid");
    expect(grid.closest(".hidden")).toBeNull();
  });

  it("卡片「去配置」→ 设置页 python 输入框获得焦点并蓝框闪烁", async () => {
    renderShell();
    const gotoBtn = screen.getByTestId("env-python-goto-config");
    await userEvent.click(gotoBtn);

    const input = await waitFor(() => {
      const el = screen.getByTestId("config-app.python.path") as HTMLInputElement;
      expect(el).toHaveFocus();
      return el;
    });
    expect(input.className).toContain("config-flash");
    // 设置页已激活：滑杆祖先不再是 hidden
    expect(screen.getByRole("slider").closest(".hidden")).toBeNull();
  });
});

describe("PathText（双击全选）", () => {
  it("双击触发选区全选，空值不绑定选择行为", async () => {
    const { PathText } = await import("@/components/ui/PathText");
    const selection = { removeAllRanges: vi.fn(), addRange: vi.fn() };
    vi.spyOn(window, "getSelection").mockReturnValue(selection as unknown as Selection);

    const { getByTestId } = render(
      <div>
        <PathText value="/data/app/~~x/lib/arm64" testid="path-full" />
        <PathText value={null} testid="path-empty" />
      </div>,
    );
    await userEvent.dblClick(getByTestId("path-full"));
    expect(selection.removeAllRanges).toHaveBeenCalled();
    expect(selection.addRange).toHaveBeenCalled();
    // 可选中样式（user-select:all 的类钩子）
    expect(getByTestId("path-full").className).toContain("path-selectable");
    // 空值渲染占位且没有 selectable 类
    expect(getByTestId("path-empty").textContent).toBe("—");
    expect(getByTestId("path-empty").className).not.toContain("path-selectable");
  });
});
