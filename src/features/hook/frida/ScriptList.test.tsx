/**
 * B 区脚本条的显示口径（第九十六轮：编辑器落地之后的交互更新）。
 *
 * 显示口径（第九十二轮用户反馈，不变）：**名字才是主角**——行内只有名字；
 * 大小 / 修改时间 / 完整路径收进 hover 气泡（ScriptMeta 直接渲染来验）。
 *
 * 交互口径（第九十六轮新增，钉住这次改版）：
 * - **单击一行 = 在编辑器里打开**（onOpen）——编辑是这一层的主线；
 * - **▶ = 起会话**（onStart）——双击不再启动（与"单击打开"打架，二选一取单击）；
 * - **脏标**：dirtyNames 里有这个名字时，行上出现 ●（未保存的改动要看得见）。
 *
 * ⚠️ "鼠标移上去气泡真的弹出来"这条不在这里测：jsdom 没有排版，Radix 浮层
 * 一挂载就要 ~14 s（@floating-ui 在 jsdom 里把 getComputedStyle 当循环用）。
 * 一条要跑 14 秒、且测的是浮层库而不是我们的判据的测试没有价值——
 * 气泡的弹出归真机手工矩阵（docs/frida_terminal_phases.md §5）。
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { I18nProvider } from "@/i18n/I18nProvider";
import type { JsFileDto } from "@/api/hook";
import { ScriptList, ScriptMeta, formatMtime, formatSize } from "./ScriptList";

const { SCRIPT, scriptWrite } = vi.hoisted(() => ({
  SCRIPT: {
    name: "hook_wechat_dexdump_and_rpc_bridge_v3.js",
    path: "/Users/me/hooks/hook_wechat_dexdump_and_rpc_bridge_v3.js",
    size: 2048,
    mtime: 1_700_000_000,
  } as JsFileDto,
  scriptWrite: vi.fn(async () => 12),
}));

vi.mock("@/api/hook", () => ({
  hookApi: { jsList: vi.fn(async () => [SCRIPT]), scriptWrite },
}));

// 选目录用原生对话框；测试里既不弹也不该弹
vi.mock("@/api/dialog", () => ({
  pickDirectory: vi.fn(async () => null),
}));

function withinProviders(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <TooltipProvider delayDuration={0}>
      <QueryClientProvider client={qc}>
        <I18nProvider>{ui}</I18nProvider>
      </QueryClientProvider>
    </TooltipProvider>,
  );
}

/** 这版 ScriptList 的工作目录由外部持有（编辑器要同一份真相） */
function list(props: Partial<Parameters<typeof ScriptList>[0]> = {}) {
  return withinProviders(
    <ScriptList
      workdir="/Users/me/hooks"
      onWorkdirChange={vi.fn()}
      onStart={vi.fn()}
      onOpen={vi.fn()}
      refreshSignal={0}
      {...props}
    />,
  );
}

describe("ScriptList：名字在行里，大小与时间在气泡里", () => {
  it("行内只写名字：长名字整行归名字，不再挤大小与时间", async () => {
    list();
    const name = await screen.findByText(SCRIPT.name);
    const row = screen.getByTestId(`script-row-${SCRIPT.name}`);

    // 这一条的全部文字就是名字（外加 hover 才现身的「启动」按钮）
    expect(row.textContent).toBe(`${SCRIPT.name}启动`);
    expect(row).not.toHaveTextContent(formatSize(SCRIPT.size));
    expect(row).not.toHaveTextContent(formatMtime(SCRIPT.mtime));
    // 名字占满整行：既不再 truncate，也不靠 title 藏全名
    expect(name).not.toHaveAttribute("title");
    expect(name.className).toContain("line-clamp-2");
    expect(name.className).not.toContain("truncate");
  });

  it("那条就是气泡的触发器（Radix 在 trigger 上打 data-state）", async () => {
    list();
    await screen.findByText(SCRIPT.name);
    const row = screen.getByTestId(`script-row-${SCRIPT.name}`);
    expect(row).toHaveAttribute("data-state", "closed");
    // 弹出本身（closed → delayed-open）见文件头：jsdom 下测它等于测浮层库
  });

  it("气泡内容收着完整名字、路径、大小、修改时间与点击语义", () => {
    withinProviders(<ScriptMeta file={SCRIPT} />);
    const bubble = screen.getByText(SCRIPT.name).closest("div");
    expect(bubble).toHaveTextContent(SCRIPT.path);
    expect(bubble).toHaveTextContent(formatSize(SCRIPT.size)); // 2.0 KB
    expect(bubble).toHaveTextContent(formatMtime(SCRIPT.mtime)); // 2023-11-15 06:13（随本机时区）
    expect(bubble).toHaveTextContent("点一行在编辑器里打开");
  });
});

describe("交互：单击打开、▶ 启动、脏标可见", () => {
  it("单击一行 = 打开（onOpen），不起会话", async () => {
    const onOpen = vi.fn();
    const onStart = vi.fn();
    list({ onOpen, onStart });
    await screen.findByText(SCRIPT.name);
    fireEvent.click(screen.getByTestId(`script-row-${SCRIPT.name}`));
    expect(onOpen).toHaveBeenCalledWith(SCRIPT);
    expect(onStart).not.toHaveBeenCalled();
  });

  it("▶ = 起会话，且不触发「打开」（事件停在了按钮上）", async () => {
    const onOpen = vi.fn();
    const onStart = vi.fn();
    list({ onOpen, onStart });
    await screen.findByText(SCRIPT.name);
    const row = screen.getByTestId(`script-row-${SCRIPT.name}`);
    fireEvent.click(within(row).getByText("启动"));
    await waitFor(() => expect(onStart).toHaveBeenCalledWith(SCRIPT));
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("双击不再启动会话（单击已经占住「打开」；两个手势各表一义）", async () => {
    const onStart = vi.fn();
    list({ onStart });
    await screen.findByText(SCRIPT.name);
    fireEvent.doubleClick(screen.getByTestId(`script-row-${SCRIPT.name}`));
    await new Promise((r) => setTimeout(r, 20));
    expect(onStart).not.toHaveBeenCalled();
  });

  it("脏文件行上有 ●（未保存的改动不许看不见）", async () => {
    list({ dirtyNames: new Set([SCRIPT.name]) });
    await screen.findByText(SCRIPT.name);
    expect(screen.getByTestId(`script-dirty-${SCRIPT.name}`)).toBeTruthy();
  });
});

/**
 * 「新建」：脚本条自己就能建文件（不必先切到编辑器）。
 *
 * 口径：**默认只落 .js**——名字只给主体时补 `.js`；别的扩展名不静默改名，直接拒绝。
 * 建完把名字交给外层打开（onCreated），输入行收起。
 */
describe("新建：默认只落 .js", () => {
  it("点「新建」展开输入行；没写扩展名 → 补 .js 后创建，并交给外层打开", async () => {
    scriptWrite.mockClear();
    const onCreated = vi.fn();
    list({ onCreated });
    await screen.findByText(SCRIPT.name);

    fireEvent.click(screen.getByTestId("script-new"));
    const input = screen.getByTestId("script-new-name");
    fireEvent.change(input, { target: { value: "hook_new" } });
    fireEvent.click(screen.getByTestId("script-new-create"));

    await waitFor(() =>
      expect(scriptWrite).toHaveBeenCalledWith("hook_new.js", expect.stringContaining("/*"), true),
    );
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith("hook_new.js"));
    // 建完收起输入行（成功之后再问"还要不要再建一个"没有意义）
    expect(screen.queryByTestId("script-new-row")).toBeNull();
  });

  it("回车等价于「创建」；带 .js 的名字原样落盘", async () => {
    scriptWrite.mockClear();
    const onCreated = vi.fn();
    list({ onCreated });
    await screen.findByText(SCRIPT.name);

    fireEvent.click(screen.getByTestId("script-new"));
    const input = screen.getByTestId("script-new-name");
    fireEvent.change(input, { target: { value: "  hook_enter.js  " } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith("hook_enter.js"));
    expect(scriptWrite).toHaveBeenCalledWith("hook_enter.js", expect.any(String), true);
  });

  it("别的扩展名不静默改名：报名字不合法，不写盘，输入行留着让她改", async () => {
    scriptWrite.mockClear();
    list();
    await screen.findByText(SCRIPT.name);

    fireEvent.click(screen.getByTestId("script-new"));
    fireEvent.change(screen.getByTestId("script-new-name"), {
      target: { value: "hook_new.txt" },
    });
    fireEvent.click(screen.getByTestId("script-new-create"));

    await waitFor(() => expect(screen.getByText(/名字不合法/)).toBeTruthy());
    expect(scriptWrite).not.toHaveBeenCalled();
    expect(screen.getByTestId("script-new-row")).toBeTruthy();
  });

  it("Escape 收起输入行，什么都不建", async () => {
    scriptWrite.mockClear();
    list();
    await screen.findByText(SCRIPT.name);

    fireEvent.click(screen.getByTestId("script-new"));
    const input = screen.getByTestId("script-new-name");
    fireEvent.change(input, { target: { value: "hook_never" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(screen.queryByTestId("script-new-row")).toBeNull();
    expect(scriptWrite).not.toHaveBeenCalled();
  });

  it("没选工作目录时没有「新建」（没有地方落文件，不给假按钮）", () => {
    list({ workdir: "" });
    expect(screen.queryByTestId("script-new")).toBeNull();
  });
});