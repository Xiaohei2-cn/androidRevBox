import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";

/**
 * P7 仪表盘布局与剪枝契约（PHASES §10）：
 * - 两列网格，任何卡不得独占一行；「安卓前台应用」唯一整行且排最底；
 * - Frida 检测依赖 Python 就绪：未就绪时不发起 env_frida 查询（剪枝）。
 */

const deviceMocks = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
}));

const envMocks = vi.hoisted(() => ({
  python: vi.fn(),
  node: vi.fn(),
  frida: vi.fn(),
  idaMcp: vi.fn(),
  jadxMcp: vi.fn(),
  foreground: vi.fn(),
}));

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return {
    ...actual,
    deviceApi: {
      ...actual.deviceApi,
      environment: deviceMocks.environment,
      list: deviceMocks.list,
      onChanged: deviceMocks.onChanged,
    },
  };
});

vi.mock("@/api/env", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/env")>();
  return {
    ...actual,
    envApi: {
      ...actual.envApi,
      python: envMocks.python,
      node: envMocks.node,
      frida: envMocks.frida,
      idaMcp: envMocks.idaMcp,
      jadxMcp: envMocks.jadxMcp,
      foreground: envMocks.foreground,
    },
  };
});

import { DashboardPage } from "./DashboardPage";

function readyEnvMocks() {
  deviceMocks.environment.mockResolvedValue({
    installed: true,
    path: "/sdk/adb",
    source: "path_env",
    version: "1.0.41",
  });
  deviceMocks.list.mockResolvedValue([
    { serial: "emu-5554", state: "device", transport: "emulator", model: "Pixel" },
  ]);
  deviceMocks.onChanged.mockResolvedValue(() => {});
  envMocks.python.mockResolvedValue({
    configured: true,
    ready: true,
    path: "/usr/bin/python3",
    version: "3.12.4",
  });
  envMocks.node.mockResolvedValue({
    ready: true,
    path: "/usr/local/bin/node",
    version: "20.11.1",
    npmGlobalRoot: "/usr/local/lib/node_modules",
  });
  envMocks.frida.mockResolvedValue({
    pythonReady: true,
    installed: true,
    fridaVersion: "16.5.9",
    fridaToolsVersion: "13.6.1",
  });
  envMocks.idaMcp.mockResolvedValue({ reachable: true, port: 13337, hint: "在线" });
  envMocks.jadxMcp.mockResolvedValue({ reachable: false, port: 8650, hint: "未检测到" });
  envMocks.foreground.mockResolvedValue({
    state: "ready",
    serial: "emu-5554",
    package: "com.target.app",
    activity: "com.target.app.MainActivity",
    pid: "4321",
    nativeLibDir: "/data/app/~~x/lib/arm64",
    procPaths: [],
    hint: null,
    error: null,
  });
}

function renderDashboard() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>
        <DashboardPage />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("DashboardPage（P7 布局契约）", () => {
  beforeEach(() => {
    Object.values(deviceMocks).forEach((m) => m.mockReset());
    Object.values(envMocks).forEach((m) => m.mockReset());
    readyEnvMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("网格为两列布局，前台应用卡整行(col-span-2)且排最底", async () => {
    renderDashboard();
    const grid = screen.getByTestId("dashboard-grid");
    expect(grid.className).toContain("grid-cols-2");

    await waitFor(() =>
      expect(screen.getByTestId("fg-package")).toHaveTextContent("com.target.app"),
    );

    const wrapper = screen.getByTestId("foreground-card-wrapper");
    expect(wrapper.className).toContain("col-span-2");
    // 最底 = 是网格的最后一个元素子节点
    const elementChildren = Array.from(grid.children).filter(
      (c) => c.tagName === "DIV",
    );
    expect(elementChildren[elementChildren.length - 1]).toBe(wrapper);
  });

  it("卡片顺序：先系统(ADB)→环境(Python/Node)→工具(Frida/IDA/jadx)→前台应用最后", async () => {
    renderDashboard();
    await waitFor(() =>
      expect(screen.getByTestId("env-frida-status")).toHaveTextContent("已安装"),
    );
    const grid = screen.getByTestId("dashboard-grid");
    const texts = Array.from(grid.children).map((c) => c.textContent ?? "");
    const indexOf = (needle: string) =>
      texts.findIndex((t) => t.includes(needle));
    const adb = indexOf("ADB 环境");
    const python = indexOf("Python 环境");
    const node = indexOf("Node 环境");
    const frida = indexOf("Frida");
    const ida = indexOf("IDA");
    const jadx = indexOf("jadx-gui");
    const fg = texts.findIndex((t) => t.includes("安卓前台应用"));
    expect(adb).toBeLessThan(python);
    expect(python).toBeLessThan(node);
    expect(node).toBeLessThan(frida);
    expect(frida).toBeLessThan(ida);
    expect(ida).toBeLessThan(jadx);
    expect(jadx).toBeLessThan(fg);
    expect(fg).toBe(texts.length - 1);
  });

  it("剪枝：Python 未就绪时 Frida 卡显示待命且不发起 env_frida 查询", async () => {
    envMocks.python.mockResolvedValue({
      configured: false,
      ready: false,
      hint: "未配置 Python 解释器",
    });
    renderDashboard();
    await waitFor(() =>
      expect(screen.getByTestId("env-frida-status")).toHaveTextContent("待 Python 就绪"),
    );
    await waitFor(() =>
      expect(screen.getByTestId("env-python-status")).toHaveTextContent("未配置"),
    );
    expect(envMocks.frida).not.toHaveBeenCalled();
  });

  it("Python 就绪时 Frida 查询放行并显示版本", async () => {
    renderDashboard();
    await waitFor(() =>
      expect(screen.getByTestId("env-frida-status")).toHaveTextContent("已安装"),
    );
    expect(screen.getByTestId("env-frida-version")).toHaveTextContent("16.5.9");
    expect(envMocks.frida).toHaveBeenCalled();
  });

  it("MCP 卡连不上显示「未检测到」而非错误态", async () => {
    renderDashboard();
    await waitFor(() =>
      expect(screen.getByTestId("env-jadx-status")).toHaveTextContent("未检测到"),
    );
    expect(screen.getByTestId("env-ida-status")).toHaveTextContent("在线 · 13337");
  });

  it("前台应用卡展示包名/Activity/PID/lib 目录", async () => {
    renderDashboard();
    await waitFor(() =>
      expect(screen.getByTestId("fg-package")).toHaveTextContent("com.target.app"),
    );
    expect(screen.getByTestId("fg-activity")).toHaveTextContent(
      "com.target.app.MainActivity",
    );
    expect(screen.getByTestId("fg-pid")).toHaveTextContent("4321");
    expect(screen.getByTestId("fg-libdir")).toHaveTextContent("/data/app/~~x/lib/arm64");
  });
});
