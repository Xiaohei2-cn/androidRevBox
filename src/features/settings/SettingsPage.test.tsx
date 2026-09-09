import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { AppProviders } from "@/app/providers";
import { SettingsPage } from "./SettingsPage";

const systemMocks = vi.hoisted(() => ({ ping: vi.fn() }));
const configMocks = vi.hoisted(() => ({ snapshot: vi.fn(), set: vi.fn() }));

vi.mock("@/api/system", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/system")>();
  return { ...actual, systemApi: { ...actual.systemApi, ping: systemMocks.ping } };
});

vi.mock("@/api/config", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/config")>();
  return {
    ...actual,
    configApi: { ...actual.configApi, snapshot: configMocks.snapshot, set: configMocks.set },
  };
});

describe("SettingsPage", () => {
  beforeEach(() => {
    systemMocks.ping.mockReset();
    systemMocks.ping.mockResolvedValue({
      appVersion: "0.1.0",
      tauriVersion: "2.8.0",
      os: "macos",
      arch: "arm64",
    });
    configMocks.snapshot.mockReset();
    configMocks.snapshot.mockResolvedValue([]);
    configMocks.set.mockReset();
    configMocks.set.mockResolvedValue(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("渲染三个主题选项", () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    for (const label of ["浅色", "深色", "跟随系统"]) {
      expect(screen.getByRole("radio", { name: label })).toBeInTheDocument();
    }
  });

  it("点击主题选项后 radiogroup 选中态更新", async () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    const dark = screen.getByRole("radio", { name: "深色" });
    await userEvent.click(dark);
    expect(dark).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: "跟随系统" }),
    ).toHaveAttribute("aria-checked", "false");
  });

  it("透明度滑杆存在且初始值为 100", () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    expect(screen.getByRole("slider")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });

  it("关于小节显示应用版本/Tauri 版本/运行平台（自仪表盘迁入，P7）", async () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("about-app-version")).toHaveTextContent("0.1.0"),
    );
    expect(screen.getByTestId("about-tauri-version")).toHaveTextContent("2.8.0");
    expect(screen.getByTestId("about-platform")).toHaveTextContent("macos · arm64");
  });

  it("工具环境区提供 python/node/MCP 端口配置输入", () => {
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    expect(screen.getByTestId("config-app.python.path")).toBeInTheDocument();
    expect(screen.getByTestId("config-app.node.path")).toBeInTheDocument();
    expect(screen.getByTestId("config-app.tools.ida_mcp_port")).toBeInTheDocument();
    expect(screen.getByTestId("config-app.tools.jadx_mcp_port")).toBeInTheDocument();
  });
});
