import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n/I18nProvider";

/**
 * AR9.4 · M1 页面回归（可自动化的那半边）。
 *
 * 阶段文档要求逐页回归，并要求「非 root 设备必须明确显示 Zygisk capability 缺失」。
 * 设备侧那半边由 18 条真机腿覆盖；这一份覆盖的是**界面真的把 typed 结果画出来了**——
 * 因为 D043 那类缺陷（前端按 camelCase 抄协议字段、wire 其实是 snake_case）
 * 在 typecheck、Rust 测试、真机腿里全都不会红，只有渲染一次才暴露。
 *
 * 所以这里的每个 fixture 都按 Rust 的真实序列化形状写（snake_case），
 * 而不是按前端接口「看起来应该」的样子写。
 */

const api = vi.hoisted(() => ({
  environment: vi.fn(),
  list: vi.fn(),
  onChanged: vi.fn(),
  ls: vi.fn(),
  fileStat: vi.fn(),
  filePreview: vi.fn(),
  pkgLibDir: vi.fn(),
  soReplace: vi.fn(),
  launch: vi.fn(),
  forceStop: vi.fn(),
  uninstall: vi.fn(),
  foreground: vi.fn(),
  zygiskList: vi.fn(),
  zygiskStatus: vi.fn(),
  fridaServerStatus: vi.fn(),
  fridaServerStart: vi.fn(),
  fridaServerStop: vi.fn(),
}));

vi.mock("@/api/device", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/device")>();
  return {
    ...actual,
    deviceApi: {
      ...actual.deviceApi,
      environment: api.environment,
      list: api.list,
      onChanged: api.onChanged,
      ls: api.ls,
      fileStat: api.fileStat,
      filePreview: api.filePreview,
      pkgLibDir: api.pkgLibDir,
      soReplace: api.soReplace,
      launch: api.launch,
      forceStop: api.forceStop,
      uninstall: api.uninstall,
      fridaServerStatus: api.fridaServerStatus,
      fridaServerStart: api.fridaServerStart,
      fridaServerStop: api.fridaServerStop,
    },
  };
});
vi.mock("@/api/zygisk", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/zygisk")>();
  return {
    ...actual,
    zygiskApi: { ...actual.zygiskApi, list: api.zygiskList, status: api.zygiskStatus },
  };
});
vi.mock("@/api/env", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api/env")>();
  return { ...actual, envApi: { ...actual.envApi, foreground: api.foreground } };
});
vi.mock("@/app/nav", () => ({
  useAppNav: () => ({ tab: "devices", subTab: "files", go: vi.fn(), goAdbSubTab: vi.fn() }),
  useActiveTab: () => true,
}));
vi.mock("@/hooks/useDragDropPath", () => ({ useDragDropPath: vi.fn() }));

import { AppsView, FilesView } from "@/features/devices/DevicesPage";
import { SoReplacePage } from "@/features/binary/SoReplacePage";
import { FridaServerControl } from "@/features/hook/frida/SettingsPanel";

function renderWith(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider>{ui}</I18nProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.environment.mockResolvedValue({ installed: true, path: "/sdk/adb", source: "path_env" });
  api.list.mockResolvedValue([{ serial: "SERIAL-1", state: "device", transport: "usb" }]);
  api.onChanged.mockResolvedValue(() => {});
});

describe("文件页（AR7.1 typed DTO 的渲染证据）", () => {
  it("把 wire 的 mode_text / mtime_unix 真的画出来，而不是空白", async () => {
    // device_file_list 走的是本地 FileEntry（camelCase），别和协议 DTO 混了
    api.ls.mockResolvedValue([
      {
        name: "frida-server",
        isDir: false,
        size: 53539200,
        symlink: null,
        perms: "-rwxr-xr-x",
      },
    ]);
    api.fileStat.mockResolvedValue({
      requested_path: "/data/local/tmp/frida-server",
      path: "/data/local/tmp/frida-server",
      stat: {
        name: "frida-server",
        kind: "file",
        mode: 0o755,
        mode_text: "-rwxr-xr-x",
        uid: 2000,
        gid: 2000,
        size: 53539200,
        mtime_unix: 1755000000,
        readable: true,
      },
    });
    renderWith(<FilesView serial="SERIAL-1" />);
    // 权限位：D043 之前这里是 undefined（界面空白），现在必须出现原始权限串
    // 目录行里的权限串（本地模型）
    const entry = await screen.findByText("frida-server");
    entry.closest("button")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    // 元数据行来自协议 DTO：mode_text / mtime_unix 必须真的渲染出来（D043 的回归点）
    await waitFor(() => expect(document.body.textContent).toContain("uid:gid 2000:2000"));
    // 列表行（本地 FileEntry）+ 元数据行（协议 DTO 的 mode_text）各一处
    const hits = (document.body.textContent ?? "").split("-rwxr-xr-x").length - 1;
    expect(hits).toBeGreaterThanOrEqual(2);
    // mtime 也必须真的格式化出来：undefined 会渲染成 "NaN"/"Invalid"
    expect(document.body.textContent).toContain("mtime ");
    expect(document.body.textContent).not.toMatch(/NaN|Invalid Date/);
  });
});

describe("应用页 · 非 root / 无模块设备（M1 明确要求）", () => {
  it("清单失败时必须说清是 Zygisk 缺失，而不是给一张空表", async () => {
    api.zygiskList.mockRejectedValue(
      new Error("package.list_localized 需要 Zygisk 模块（zygisk_applist / applistpro）"),
    );
    api.zygiskStatus.mockResolvedValue({
      lifecycle: "faulted",
      bridge_ready: false,
      root_available: false,
      sub_protocol_version: 0,
      module_id: null,
      detail: "root 不可用且两个模块端口均未响应，无法区分未安装/未启用/需重启",
    });
    renderWith(<AppsView serial="SERIAL-1" />);
    await waitFor(() => expect(screen.getByText(/无法区分未安装|未安装/)).toBeTruthy());
    expect(document.body.textContent).toContain("root 不可用且两个模块端口均未响应");
    // 关键反向断言：不能出现「共 0 个应用」这种冒充成功的空态
    expect(screen.queryByText(/共 0 个/)).toBeNull();
  });

  it("强停返回 replayed 时不能显示成一次新的成功", async () => {
    api.zygiskList.mockResolvedValue({
      items: [
        {
          packageName: "com.x",
          label: "示例应用",
          versionName: "1.0",
          versionCode: 1,
          labelSource: "module",
          requestedLocale: "zh-CN",
          resolvedLocale: "zh-CN",
          fallbackReason: null,
          uid: 10123,
          isSystem: false,
          enabled: true,
        },
      ],
      requestedLocale: "zh-CN",
      resolvedLocale: "zh-CN",
      warnings: [],
    });
    api.zygiskStatus.mockResolvedValue({
      lifecycle: "bridge_ready",
      bridge_ready: true,
      root_available: true,
      sub_protocol_version: 2,
      module_id: "applistpro",
      detail: null,
    });
    api.forceStop.mockResolvedValue({
      action: "force_stop",
      package: "com.x",
      operation_id: "force-stop-1",
      outcome: "replayed",
      verified: true,
      ran_as_root: false,
    });
    renderWith(<AppsView serial="SERIAL-1" />);
    // 先选中清单里的那一行，否则三个写操作按钮都是 disabled
    const row = await screen.findByText("示例应用");
    row.closest("button")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    const stop = await screen.findByText("强停");
    stop.closest("button")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await waitFor(() => expect(api.forceStop).toHaveBeenCalledWith("SERIAL-1", "com.x"));
    // 幂等命中必须被单独说出来
    await waitFor(() => expect(document.body.textContent).toContain("本次没有再动设备"));
  });
});

describe("SO 替换页（AR8.4 typed 步骤链）", () => {
  it("按 wire 键名渲染目标路径与步骤链", async () => {
    api.pkgLibDir.mockResolvedValue("/data/app/~~a==/com.x-b==/lib/arm64");
    api.soReplace.mockResolvedValue({
      package: "com.x",
      target_path: "/data/app/~~a==/com.x-b==/lib/arm64/libfoo.so",
      staged_path: "/data/local/tmp/app-reverse-tools-so/com.x-1f2e3d4c/libfoo.so",
      operation_id: "so-replace-1",
      outcome: "executed",
      verified: true,
      replaced_existing: false,
      steps: [
        { name: "validate_input", ok: true },
        { name: "verify_staged", ok: true, detail: "sha256=0347e8c2be15… 一致" },
        { name: "install", ok: true, detail: "/data/app/…/libfoo.so" },
      ],
      detail: "sha256_matched",
    });
    renderWith(<SoReplacePage />);
    const run = await screen.findByTestId("so-replace-run");
    fireEvent.change(screen.getByLabelText("本地文件"), {
      target: { value: "/tmp/libfoo.so" },
    });
    fireEvent.change(screen.getByLabelText("包名"), { target: { value: "com.x" } });
    await waitFor(() => expect(run).toBeEnabled());
    run.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await waitFor(() => expect(api.soReplace).toHaveBeenCalled());
    await waitFor(() =>
      expect(document.body.textContent).toContain("/data/app/~~a==/com.x-b==/lib/arm64/libfoo.so"),
    );
    expect(
      await screen.findByTestId("so-replace-step-verify_staged"),
    ).toBeTruthy();
    expect(document.body.textContent).toContain("sha256=0347e8c2be15");
  });
});

describe("Frida 工作台 · 设备侧服务（AR9.1）", () => {
  it("running_as_shell 必须说明「连得上但注入不了别人」", async () => {
    api.fridaServerStatus.mockResolvedValue({
      state: "running_as_shell",
      running: true,
      as_root: false,
      binary_name: "frida-server",
      pid: 4321,
      uid: 2000,
      listen_address: "127.0.0.1",
      port: 27042,
      listening: true,
      version: "17.17.0",
      detail: "frida-server 以 uid=2000 运行：连得上但注入不了其它进程",
    });
    renderWith(<FridaServerControl serial="SERIAL-1" t={(k, v) => `${k}:${String(v?.uid ?? "")}`} />);
    await waitFor(() => expect(document.body.textContent).toContain("asShell"));
    expect(document.body.textContent).toContain("2000");
  });
});

describe("应用页 · 默认三方 + 一个眼睛开关 + 本地搜索", () => {
  /**
   * 这一组断言盯的是三件容易做歪的事：
   * ① 默认必须只请求三方清单（scope=user），别一上来就把几百个系统包倒进列表；
   * ② "要不要含系统应用"只有一个按钮（眼睛），它切的是**请求范围**；
   * ③ 搜索是本地过滤——边打字边打设备是假的"搜索功能"，
   *    所以这里反向断言 `zygisk.list` 的调用次数不随输入增长。
   */
  const userItem = {
    packageName: "com.amazon.mShop.android.shopping",
    label: "亚马逊购物",
    versionName: "32.17.0.100",
    versionCode: 1,
    labelSource: "framework",
    requestedLocale: "zh-CN",
    resolvedLocale: "zh-CN",
    fallbackReason: null,
    uid: 10214,
    isSystem: false,
    enabled: true,
  };
  const systemItem = {
    packageName: "com.android.settings",
    label: "设置",
    versionName: "14",
    versionCode: 34,
    labelSource: "framework",
    requestedLocale: "zh-CN",
    resolvedLocale: "zh-CN",
    fallbackReason: null,
    uid: 1000,
    isSystem: true,
    enabled: true,
  };

  beforeEach(() => {
    // mock 按 scope 返回不同范围：照实模拟设备端行为，而不是把所有包一股脑给出去
    api.zygiskList.mockImplementation((_serial: string, scope: string) =>
      Promise.resolve({
        items: scope === "user" ? [userItem] : [userItem, systemItem],
        successCount: scope === "user" ? 1 : 2,
        fallbackCount: 0,
        warnings: [],
        deviceLocale: "zh-CN",
        channel: "zygisk_v2",
      }),
    );
    api.zygiskStatus.mockResolvedValue({
      lifecycle: "bridge_ready",
      bridge_ready: true,
      root_available: true,
      sub_protocol_version: 2,
      module_id: "applistpro",
      detail: null,
    });
  });

  it("默认只请求三方清单，系统应用不在列表里", async () => {
    renderWith(<AppsView serial="SERIAL-1" />);
    await waitFor(() => expect(screen.getByText("亚马逊购物")).toBeTruthy());
    expect(api.zygiskList).toHaveBeenCalledWith("SERIAL-1", "user", expect.anything());
    expect(screen.queryByText("设置")).toBeNull();
  });

  it("眼睛按钮切换的是请求范围：点开才把系统应用一起取回", async () => {
    renderWith(<AppsView serial="SERIAL-1" />);
    await waitFor(() => expect(screen.getByText("亚马逊购物")).toBeTruthy());
    const calls = api.zygiskList.mock.calls.length;
    fireEvent.click(screen.getByTestId("apps-toggle-system"));
    await waitFor(() => expect(screen.getByText("设置")).toBeTruthy());
    expect(api.zygiskList).toHaveBeenLastCalledWith("SERIAL-1", "all", expect.anything());
    // 一个按钮来回切，不该多出第三个入口
    fireEvent.click(screen.getByTestId("apps-toggle-system"));
    await waitFor(() => expect(screen.queryByText("设置")).toBeNull());
    expect(api.zygiskList).toHaveBeenLastCalledWith("SERIAL-1", "user", expect.anything());
    expect(api.zygiskList.mock.calls.length).toBe(calls + 2);
  });

  it("搜索是本地过滤：打字不发请求，只收窄列表", async () => {
    renderWith(<AppsView serial="SERIAL-1" />);
    // 工具条是清单到位之后才渲染的，先等它出现再点（否则这条测试是在测加载顺序）
    await waitFor(() => expect(screen.getByTestId("apps-toggle-system")).toBeTruthy());
    fireEvent.click(screen.getByTestId("apps-toggle-system"));
    await waitFor(() => expect(screen.getByText("设置")).toBeTruthy());
    const calls = api.zygiskList.mock.calls.length;

    fireEvent.change(screen.getByTestId("apps-search"), { target: { value: "设置" } });
    expect(screen.getByText("设置")).toBeTruthy();
    expect(screen.queryByText("亚马逊购物")).toBeNull();
    expect(api.zygiskList.mock.calls.length).toBe(calls);
    expect(screen.getByTestId("apps-count").textContent).toContain("1");

    // 包名也能搜
    fireEvent.change(screen.getByTestId("apps-search"), {
      target: { value: "mShop" },
    });
    expect(screen.getByText("亚马逊购物")).toBeTruthy();
    expect(screen.queryByText("设置")).toBeNull();
    expect(api.zygiskList.mock.calls.length).toBe(calls);

    fireEvent.click(screen.getByTestId("apps-search-clear"));
    expect(screen.getByText("设置")).toBeTruthy();
    expect(screen.getByText("亚马逊购物")).toBeTruthy();
  });

  it("三方范围里搜不到的东西要说明可能被藏起来了，而不是空列表了事", async () => {
    renderWith(<AppsView serial="SERIAL-1" />);
    await waitFor(() => expect(screen.getByText("亚马逊购物")).toBeTruthy());
    fireEvent.change(screen.getByTestId("apps-search"), { target: { value: "settings" } });
    expect(screen.getByText("没有匹配的应用")).toBeTruthy();
    // 关键：提示要说清"可能是范围问题"，给用户下一步动作
    expect(screen.getByText(/眼睛/)).toBeTruthy();
    fireEvent.click(screen.getByTestId("apps-toggle-system"));
    await waitFor(() => expect(screen.getByText("设置")).toBeTruthy());
  });
});
