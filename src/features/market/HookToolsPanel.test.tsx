/**
 * 工具市场 · hook 工具（P13）的界面契约。
 *
 * 钉三件事，都是"改布局时最容易悄悄破掉"的：
 * 1. 左侧是**气泡卡片**（button 且带 `market-tool-<id>`），不是表格行——这是用户明确要的形态；
 * 2. 点一张卡片，右侧换成那个工具的说明与官方地址（选中态与详情必须是同一个东西）；
 * 3. 安装按钮只在有设备时可用；装完把**设备路径与复核结果**如实显示出来
 *    （`verifiedBytes === null` 与"验了但不一致"是两种话，不能混）。
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { HookToolsPanel } from "./HookToolsPanel";
import { I18nProvider } from "@/i18n/I18nProvider";
import { marketApi } from "@/api/market";
import { deviceApi } from "@/api/device";

const TOOLS = [
  {
    id: "stackplz",
    name: "stackplz",
    tagline: "syscall 与硬件断点追踪",
    description: "基于 eBPF 的追踪工具",
    homepage: "https://github.com/SeeFlowerX/stackplz",
    license: "GPL-3.0",
    version: "v3.0.3",
    downloadUrl: "https://example.invalid/stackplz",
    sizeBytes: 8_975_584,
    sha256: "a".repeat(64),
    deviceFileName: "stackplz",
    deviceAbi: ["arm64-v8a"],
    usage: ["stackplz -n com.example.app"],
  },
  {
    id: "other",
    name: "other-tool",
    tagline: "另一个工具",
    description: "另一段说明",
    homepage: "https://example.invalid/other",
    license: "MIT",
    version: "v1.0.0",
    downloadUrl: "https://example.invalid/other.bin",
    sizeBytes: 1024,
    sha256: "b".repeat(64),
    deviceFileName: "other",
    deviceAbi: [],
    usage: ["other --help"],
  },
];

const REPORT = {
  id: "stackplz",
  version: "v3.0.3",
  serial: "PIXEL-4",
  devicePath: "/data/local/tmp/stackplz",
  bytes: 8_975_584,
  sha256: "a".repeat(64),
  verifiedBytes: 8_975_584,
  executable: true,
};

vi.mock("@/api/market", async () => {
  const actual = await vi.importActual<typeof import("@/api/market")>("@/api/market");
  return {
    ...actual,
    marketApi: {
      tools: vi.fn(async () => TOOLS),
      deviceAbi: vi.fn(async () => "arm64-v8a"),
      install: vi.fn(async () => REPORT),
    },
  };
});

vi.mock("@/api/device", async () => {
  const actual = await vi.importActual<typeof import("@/api/device")>("@/api/device");
  return {
    ...actual,
    deviceApi: {
      ...actual.deviceApi,
      list: vi.fn(async () => [
        { serial: "PIXEL-4", state: "device", transport: "usb", model: "Pixel 4" },
      ]),
    },
  };
});

function renderPanel() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <I18nProvider>
        <HookToolsPanel />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("工具市场 hook 工具", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("左侧每一项都是气泡按钮，默认选中第一个并显示它的官方地址", async () => {
    renderPanel();
    const first = await screen.findByTestId("market-tool-stackplz");
    // 气泡 = 整块可点的 button（不是 li / 表格行）
    expect(first.tagName).toBe("BUTTON");
    expect(first.getAttribute("data-selected")).toBe("true");
    // 选中项与右侧详情必须是同一个：官方地址来自被选中的那一项
    expect((await screen.findByTestId("market-homepage")).textContent).toBe(
      TOOLS[0].homepage,
    );
    // 版本号在气泡卡片和详情里各出现一次（两处都该有），限定在详情里断言
    expect(screen.getByTestId("market-detail").textContent).toContain("v3.0.3");
  });

  it("点另一张卡片，右侧换成另一个工具的说明与地址", async () => {
    renderPanel();
    const second = await screen.findByTestId("market-tool-other");
    fireEvent.click(second);
    await waitFor(() => {
      expect(screen.getByTestId("market-homepage").textContent).toBe(TOOLS[1].homepage);
    });
    expect(second.getAttribute("data-selected")).toBe("true");
    expect(screen.getByText("另一段说明")).toBeTruthy();
  });

  it("安装后显示设备路径与复核结果", async () => {
    renderPanel();
    await screen.findByTestId("market-tool-stackplz");
    // 设备列表到达后自动选中那台在线设备
    await waitFor(() => {
      const button = screen.getByTestId("market-install") as HTMLButtonElement;
      expect(button.disabled).toBe(false);
    });
    fireEvent.click(screen.getByTestId("market-install"));
    const report = await screen.findByTestId("market-report");
    expect(report.textContent).toContain("/data/local/tmp/stackplz");
    // 复核成功要显示具体体积，而不是只说"成功"
    expect(report.textContent).toContain("8.6 MB");
    expect(marketApi.install).toHaveBeenCalledWith("PIXEL-4", "stackplz");
  });

  it("复核没做成时说的是'没做成'，不是'已复核'", async () => {
    vi.mocked(marketApi.install).mockResolvedValueOnce({
      ...REPORT,
      verifiedBytes: null,
      executable: false,
    });
    renderPanel();
    await screen.findByTestId("market-tool-stackplz");
    fireEvent.click(await screen.findByTestId("market-install"));
    const report = await screen.findByTestId("market-report");
    // 路径照样给出（文件确实推了），但复核那一格必须是否定说法
    expect(report.textContent).toContain("/data/local/tmp/stackplz");
    expect(report.textContent).not.toContain("已复核 8.6 MB");
  });

  it("设备 ABI 不匹配时禁用安装并说明原因", async () => {
    vi.mocked(marketApi.deviceAbi).mockResolvedValueOnce("x86_64");
    renderPanel();
    await screen.findByTestId("market-tool-stackplz");
    await waitFor(() => {
      const button = screen.getByTestId("market-install") as HTMLButtonElement;
      expect(button.disabled).toBe(true);
    });
    expect(screen.getByText(/x86_64/)).toBeTruthy();
  });

  it("没有设备时安装按钮不可用", async () => {
    vi.mocked(deviceApi.list).mockResolvedValueOnce([]);
    renderPanel();
    await screen.findByTestId("market-tool-stackplz");
    const button = screen.getByTestId("market-install") as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });
});
