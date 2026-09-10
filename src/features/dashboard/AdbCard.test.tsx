import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";

const mocks = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
}));

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return {
    ...actual,
    deviceApi: {
      ...actual.deviceApi,
      environment: mocks.environment,
      list: mocks.list,
      onChanged: mocks.onChanged,
    },
  };
});

import { AdbCard } from "./AdbCard";

function renderCard() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>
        <AdbCard />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("AdbCard", () => {
  beforeEach(() => {
    mocks.environment.mockReset();
    mocks.list.mockReset();
    mocks.onChanged.mockReset();
    mocks.onChanged.mockResolvedValue(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("adb 就绪：显示状态、版本与设备数", async () => {
    mocks.environment.mockResolvedValue({
      installed: true,
      path: "/sdk/platform-tools/adb",
      source: "android_home",
      version: "1.0.41",
      build: "37.0.0-mock",
    });
    mocks.list.mockResolvedValue([
      { serial: "emulator-5554", state: "device", transport: "emulator", model: "Pixel_7" },
      { serial: "abc123", state: "unauthorized", transport: "usb", model: "" },
    ]);
    renderCard();
    await waitFor(() => expect(screen.getByTestId("adb-status")).toHaveTextContent("已就绪"));
    expect(screen.getByTestId("adb-version")).toHaveTextContent("adb 1.0.41");
    // 设备列表查询在 env 解析后启用，需再等一轮
    await waitFor(() =>
      expect(screen.getByTestId("adb-device-count")).toHaveTextContent("已连接设备 1"),
    );
    // 未授权设备计入「另有 N 台」而非列表项
    expect(screen.getByText(/另有 1 台未授权\/离线/)).toBeInTheDocument();
    expect(screen.getByText("Pixel_7")).toBeInTheDocument();
  });

  it("adb 未安装：显示未检测到 + 提示文案", async () => {
    mocks.environment.mockResolvedValue({
      installed: false,
      hint: "未检测到 adb：请安装 platform-tools 或手动配置路径。",
    });
    renderCard();
    await waitFor(() => expect(screen.getByTestId("adb-status")).toHaveTextContent("未检测到"));
    expect(screen.getByTestId("adb-hint")).toHaveTextContent("platform-tools");
  });

  it("探测失败（找到但 version 出错）：显示 probeError", async () => {
    mocks.environment.mockResolvedValue({
      installed: false,
      path: "/bad/adb",
      hint: "已找到 adb 但版本探测失败",
      probeError: "adb version 退出码 Some(1)",
    });
    renderCard();
    await waitFor(() => expect(screen.getByTestId("adb-status")).toHaveTextContent("未检测到"));
    expect(screen.getByTestId("adb-hint")).toBeInTheDocument();
  });
});
