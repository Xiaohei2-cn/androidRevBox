/**
 * 算法中心的**闪烁与重绘闸门**。
 *
 * 背景（globals.css 第三轮闪烁修复）：这是透明窗口，WKWebView 里 `backdrop-filter`、
 * `transform`、opacity 关键帧动画、`position: fixed` 浮层都会给元素单独立一个合成层；
 * 层被创建/销毁的那一帧以满 alpha 实色打底，用户看到的就是"整窗先变不透明再变透明"。
 * 算法合集每次执行都会重画三栏，正好是最容易踩这条的地方，所以把它写成机器闸门，
 * 而不是靠下一个人记得这条隐性规则。
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import type { ChefBakeRequest, ChefBakeResult, ChefOperationDto } from "@/api/chef";
import { I18nProvider } from "@/i18n/I18nProvider";
import { useChefLabels } from "./labels";
import { Workbench } from "./Workbench";

const ROOT = process.cwd();
const CHEF_DIR = "src/features/crypto/chef";
const IMPLEMENTATION = [
  `${CHEF_DIR}/Catalogue.tsx`,
  `${CHEF_DIR}/RecipePanel.tsx`,
  `${CHEF_DIR}/StepArgs.tsx`,
  `${CHEF_DIR}/Workbench.tsx`,
  `${CHEF_DIR}/catalogueGroups.ts`,
  `${CHEF_DIR}/usePickDrag.ts`,
  // 指针拖拽的实现里也不许出现合成层触发器：跟随光标的假想"小卡片"就是靠 transform 定位的
  // 那种做法在这里一律不采用（见 usePickDrag.ts 顶部注释）。
  // 这轮新加的共享基础件也在透明窗口里用，一旦被别的页复用，规则同样成立
  "src/components/ui/badge.tsx",
  "src/components/ui/icon-tile.tsx",
  "src/components/ui/segmented.tsx",
  "src/components/ui/card.tsx",
  "src/components/ui/select.tsx",
  "src/components/ui/pane-header.tsx",
];

/** 会给元素单独立合成层的写法（注释里提到它们是允许的，所以先剥掉注释再查） */
const LAYER_TRIGGERS = [
  "backdrop-",
  "translate-x-",
  "translate-y-",
  "-rotate-",
  "transition-transform",
  "will-change",
  "animate-",
  "fixed ",
];

function codeOnly(file: string): string {
  return readFileSync(join(ROOT, file), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}

describe("透明窗口里的合成层约束", () => {
  it.each(IMPLEMENTATION)("%s 不使用会建合成层的写法", (file) => {
    const code = codeOnly(file);
    for (const token of LAYER_TRIGGERS) {
      expect(code, `${file} 里出现了 "${token}"：见 globals.css 第三轮闪烁修复`).not.toContain(token);
    }
  });

  it("骨架/加载态不用 opacity 关键帧（仓库里的 .skeleton 是合成器驱动的动画）", () => {
    expect(codeOnly(`${CHEF_DIR}/Catalogue.tsx`)).not.toContain('className="skeleton');
  });
});

const operations: ChefOperationDto[] = [
  {
    id: "base64Encode",
    category: "encoding",
    args: [{ name: "noWrap", kind: "bool", default: "true", options: [] }],
  },
  { id: "md5", category: "digest", args: [{ name: "uppercase", kind: "bool", default: "false", options: [] }] },
];

const chef = vi.hoisted(() => ({ operations: vi.fn(), bake: vi.fn() }));

vi.mock("@/api/chef", () => ({
  chefApi: chef,
  INPUT_ENCODINGS: ["utf8"],
  OUTPUT_DISPLAYS: ["text", "hex"],
}));

function resultFor(request: ChefBakeRequest): ChefBakeResult {
  return {
    ok: true,
    output: "SGVsbG8=",
    outputBytes: 8,
    outputUtf8: true,
    outputHex: "53 47 56",
    outputBase64: "U0dW",
    inputBytes: 5,
    failedAt: null,
    steps: request.recipe.map((step) => ({
      id: step.id,
      ok: true,
      bytes: 8,
      utf8: true,
      preview: "SGVsbG8=",
      error: null,
    })),
  };
}

function renderWorkbench() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nProvider>
        <Workbench />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  chef.operations.mockReset().mockResolvedValue(operations);
  chef.bake.mockReset().mockImplementation((request: ChefBakeRequest) =>
    Promise.resolve(resultFor(request)),
  );
});

describe("清单默认折叠", () => {
  it("首屏只有分类头，120 条算法摊开的长列表要点了才出来", async () => {
    renderWorkbench();
    await openGroups();
    const list = screen.getByLabelText("算法清单");
    expect(within(list).getByText("编码")).toBeInTheDocument();
  });

  it("默认态下条目不在 DOM 里，分类头标着 aria-expanded=false", async () => {
    renderWorkbench();
    const list = await screen.findByLabelText("算法清单");
    await within(list).findByText("编码");
    expect(within(list).queryByText("转为 Base64")).toBeNull();
    for (const header of within(list).getAllByRole("button", { expanded: undefined })) {
      if (header.getAttribute("aria-expanded") !== null) {
        expect(header.getAttribute("aria-expanded")).toBe("false");
      }
    }
  });

  it("搜索时自动展开：搜到就该看见条目", async () => {
    renderWorkbench();
    const list = await screen.findByLabelText("算法清单");
    fireEvent.change(screen.getByLabelText("算法清单搜索框"), { target: { value: "md5" } });
    expect(await within(list).findByText("MD5 摘要")).toBeInTheDocument();
  });
});

describe("执行时不重画不该动的东西", () => {
  it("labels 对象引用按 locale 稳定（否则 120 行清单每次执行都重算搜索索引）", () => {
    const client = new QueryClient();
    const { rerender, result } = renderHook(() => useChefLabels(), {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>
          <I18nProvider>{children}</I18nProvider>
        </QueryClientProvider>
      ),
    });
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
  });

  it("输出区的节点在「无结果 / 执行中 / 有结果」三个状态里始终是同一批", async () => {
    renderWorkbench();
    await openGroups();
    const pre = screen.getByTestId("chef-output");
    const status = screen.getByTestId("chef-output-status");
    fireEvent.click(within(screen.getByLabelText("算法清单")).getByRole("button", { name: /转为 Base64/ }));
    // 执行中：还没回结果，节点不该被换掉
    expect(screen.getByTestId("chef-output")).toBe(pre);
    await waitFor(() => expect(screen.getByTestId("chef-output")).toHaveTextContent("SGVsbG8="));
    expect(screen.getByTestId("chef-output")).toBe(pre);
    expect(screen.getByTestId("chef-output-status")).toBe(status);
  });

  it("同一条配方不会跑第二遍（切显示口径、焦点变化都不该重新执行）", async () => {
    renderWorkbench();
    await openGroups();
    fireEvent.click(within(screen.getByLabelText("算法清单")).getByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(chef.bake).toHaveBeenCalled());
    const calls = chef.bake.mock.calls.length;

    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    fireEvent.click(screen.getByRole("button", { name: "十六进制" }));
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(chef.bake.mock.calls.length).toBe(calls);
  });

  it("改输入仍然会重新执行（去重只挡完全相同的配方）", async () => {
    renderWorkbench();
    await openGroups();
    fireEvent.click(within(screen.getByLabelText("算法清单")).getByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(chef.bake).toHaveBeenCalled());
    const calls = chef.bake.mock.calls.length;
    fireEvent.change(screen.getByLabelText("输入"), { target: { value: "different" } });
    await waitFor(() => expect(chef.bake.mock.calls.length).toBeGreaterThan(calls));
  });
describe("拖拽期间的禁选", () => {
  it("globals.css 里 .chef-dragging 必须整树禁选（只在条目上 select-none 挡不住选区锚点漂移）", () => {
    const css = readFileSync(join(ROOT, "src/styles/globals.css"), "utf8");
    const rule = css.match(/\.chef-dragging[\s\S]*?\{([^}]*)\}/);
    expect(rule, "找不到 .chef-dragging 规则").toBeTruthy();
    expect(rule![1]).toMatch(/user-select:\s*none/);
    expect(rule![1]).toMatch(/-webkit-user-select:\s*none/);
    // 光标的 !important 不该有：改光标不建合成层，但 !important 会盖掉按钮自己的指针形态
    expect(rule![1]).toContain("cursor: grabbing");
  });
});});

/** 展开所有分类（清单默认是折叠的） */
async function openGroups() {
  fireEvent.click(await screen.findByRole("button", { name: "展开全部" }));
}