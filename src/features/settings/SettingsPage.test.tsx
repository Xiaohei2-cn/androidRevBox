import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { AppProviders } from "@/app/providers";
import { SettingsPage } from "./SettingsPage";
import { setClickThroughStatus } from "@/lib/clickThroughStatus";

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

  it("「透明区点击穿透」勾选后写回配置键，且界面立刻是选中态", async () => {
    // 写回 SQLite 只在 Tauri 环境发生（浏览器里退回 localStorage），这里得把环境装上；
    // 另外必须等水合完成——水合期间故意不写回，免得把缓存值当成用户的新设置覆盖上去
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    const box = (await screen.findByTestId("click-through-toggle")) as HTMLInputElement;
    await waitFor(() => expect(configMocks.snapshot).toHaveBeenCalled());
    expect(box.checked).toBe(false); // 默认必须关：它会改变"点下去归谁"
    await userEvent.click(box);
    expect(box.checked).toBe(true);
    await waitFor(() =>
      expect(configMocks.set).toHaveBeenCalledWith("app.settings.click_through", "true"),
    );
    await userEvent.click(box);
    await waitFor(() =>
      expect(configMocks.set).toHaveBeenLastCalledWith("app.settings.click_through", "false"),
    );
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("开关打开后给出实时读数：能分清「没生效」与「那儿本来就是实体」", async () => {
    // 用户报"透传没生效"时，开关没跑、循环卡住、判定认为那儿是实体这三种情况在界面上
    // 长得一模一样。这条读数就是用来把它们分开的，所以必须钉住它真的会跟着状态变。
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    const box = await screen.findByTestId("click-through-toggle");
    expect(screen.queryByTestId("click-through-status")).toBeNull(); // 关着不显示
    await userEvent.click(box);
    setClickThroughStatus("solid");
    const reading = await screen.findByTestId("click-through-status");
    expect(reading.textContent ?? "").toContain("实体");
    setClickThroughStatus("passing");
    const passing2 = await screen.findByTestId("click-through-status");
    expect(passing2.textContent ?? "").toContain("后面的 App");
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
