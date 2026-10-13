import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { AppProviders } from "@/app/providers";
import { SettingsPage } from "./SettingsPage";

const systemMocks = vi.hoisted(() => ({ ping: vi.fn() }));
const dialogMocks = vi.hoisted(() => ({
  pickFile: vi.fn(async () => null),
  pickDirectory: vi.fn(async () => null),
}));

vi.mock("@/api/dialog", () => ({
  pickFile: dialogMocks.pickFile,
  pickDirectory: dialogMocks.pickDirectory,
}));
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

  it("显示这一页只剩主题：透明度与点击穿透都不再是设置项", async () => {
    // 两句用户口径都在这里钉住：背景透明度"整体去掉、默认不可设置"；
    // 透明区点击穿透"是默认能力，不需要展示"。谁把它们再做回设置页，这条就红。
    render(
      <AppProviders>
        <SettingsPage />
      </AppProviders>,
    );
    expect(await screen.findByRole("radio", { name: "跟随系统" })).toBeInTheDocument();
    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.queryByTestId("click-through-toggle")).toBeNull();
    expect(screen.queryByTestId("click-through-status")).toBeNull();
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

  /**
   * 「选择目录」按钮必须真的弹**目录**选择器。
   *
   * 这里钉的是一次真实事故：原来的实现两个分支写的是同一件事（都调 pickFile），
   * 于是 kind="directory" 的「frida 类型库目录」弹的其实是选文件对话框——
   * 用户一个目录都选不中，且不报错、看起来像"点了没反应"。
   */
  describe("路径行的选择器按 kind 分派", () => {
    beforeEach(() => {
      dialogMocks.pickFile.mockClear().mockResolvedValue(null);
      dialogMocks.pickDirectory.mockClear().mockResolvedValue(null);
    });

    it("kind=directory（frida 类型库目录）→ 调 pickDirectory，不调 pickFile", async () => {
      render(
        <AppProviders>
          <SettingsPage />
        </AppProviders>,
      );
      await userEvent.click(screen.getByTestId("browse-app.hook.frida_types_dir"));
      await waitFor(() => expect(dialogMocks.pickDirectory).toHaveBeenCalledTimes(1));
      expect(dialogMocks.pickFile).not.toHaveBeenCalled();
    });

    it("kind=file（Python 解释器）→ 调 pickFile，不调 pickDirectory", async () => {
      render(
        <AppProviders>
          <SettingsPage />
        </AppProviders>,
      );
      await userEvent.click(screen.getByTestId("browse-app.python.path"));
      await waitFor(() => expect(dialogMocks.pickFile).toHaveBeenCalledTimes(1));
      expect(dialogMocks.pickDirectory).not.toHaveBeenCalled();
    });
  });
});
