/**
 * 算法合集的交互与**语言适配**测试。
 *
 * 为什么值得单独一份：算法清单的文案是动态键（`chef.op.<id>.name`），
 * 少一个词不会报错、只会退回显示 `base64Encode` 这种 id；用户切到英文界面
 * 却看到一堆中文/原始 id 是这次移植最容易犯的错，所以在这里用真渲染钉住：
 * 换语言 → 清单立刻跟着换；点/拖 → 真的进配方；参数改动 → 真的发给后端。
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import type { ChefBakeRequest, ChefBakeResult, ChefOperationDto } from "@/api/chef";
import { DICTIONARIES } from "@/i18n";
import { I18nProvider } from "@/i18n/I18nProvider";
import { MAIN_TABS } from "@/components/nav/MainTabRail";
import { CryptoPage } from "../CryptoPage";
import { Workbench } from "./Workbench";

const operations: ChefOperationDto[] = [
  {
    id: "base64Encode",
    category: "encoding",
    args: [
      { name: "urlSafe", kind: "bool", default: "false", options: [] },
      { name: "noWrap", kind: "bool", default: "true", options: [] },
    ],
  },
  {
    id: "charsetEncode",
    category: "encoding",
    args: [
      {
        name: "charset",
        kind: "choice",
        default: "utf-8",
        options: ["utf-8", "gbk", "shift_jis"],
      },
      { name: "strict", kind: "bool", default: "false", options: [] },
    ],
  },
  {
    id: "base64Decode",
    category: "encoding",
    args: [
      { name: "urlSafe", kind: "bool", default: "false", options: [] },
      { name: "strict", kind: "bool", default: "true", options: [] },
    ],
  },
  { id: "md5", category: "digest", args: [{ name: "uppercase", kind: "bool", default: "false", options: [] }] },
  {
    id: "aesEncrypt",
    category: "cipher",
    args: [
      { name: "mode", kind: "choice", default: "cbc", options: ["cbc", "ecb", "ctr"] },
      { name: "key", kind: "text", default: "", options: [] },
      { name: "padding", kind: "choice", default: "pkcs7", options: ["pkcs7", "none"] },
    ],
  },
  { id: "rot13", category: "cipher", args: [] },
];

const chef = vi.hoisted(() => ({ operations: vi.fn(), bake: vi.fn() }));

vi.mock("@/api/chef", () => ({
  chefApi: chef,
  INPUT_ENCODINGS: ["utf8", "hex", "base64"],
  OUTPUT_DISPLAYS: ["text", "hex", "base64"],
}));

/** 后端形状的假结果：步数跟着请求走，失败/二进制都可由用例指定 */
function resultFor(request: ChefBakeRequest, overrides: Partial<ChefBakeResult> = {}): ChefBakeResult {
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
    ...overrides,
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

function rect(left: number, top: number, right: number, bottom: number): DOMRect {
  return {
    left,
    top,
    right,
    bottom,
    x: left,
    y: top,
    width: right - left,
    height: bottom - top,
    toJSON: () => ({}),
  } as DOMRect;
}

/**
 * 拖拽的落点是用矩形算出来的，而 jsdom 一律返回全 0（等于"所有元素都在原点"），
 * 所以这里把配方面板与每一行的位置钉住：面板 0..420 宽、每行 60px 高、第一行从 y=40 起。
 * 不钉这个，拖拽测试只能测"事件发出去了"，测不到"插在了正确的那一步前面"。
 */
function stubRecipeGeometry() {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.getAttribute("data-testid") === "chef-recipe-area") return rect(0, 0, 420, 600);
    const index = this.getAttribute("data-step-index");
    if (index !== null) {
      const i = Number(index);
      return rect(8, 40 + i * 60, 404, 90 + i * 60);
    }
    return rect(0, 0, 0, 0);
  });
}

function catalogue(): HTMLElement {
  return screen.getByLabelText("算法清单");
}

function stepRows(): HTMLElement[] {
  return screen.queryAllByTestId("chef-step");
}

/**
 * 清单默认**全部折叠**（120 条摊开是一眼望不到头），用例要看条目就先展开。
 * 展开按钮的文案跟着语言走，所以换语言的用例要传自己的那一份。
 */
async function openCatalogue(expandLabel = "\u5c55\u5f00\u5168\u90e8") {
  fireEvent.click(await screen.findByRole("button", { name: expandLabel }));
}

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
  chef.operations.mockReset().mockResolvedValue(operations);
  chef.bake.mockReset().mockImplementation((request: ChefBakeRequest) =>
    Promise.resolve(resultFor(request)),
  );
});

describe("算法清单的语言适配", () => {
  it("默认中文界面里算法名、分类名都是中文", async () => {
    renderWorkbench();
    await openCatalogue();
    const list = await screen.findByLabelText("算法清单");
    expect(await within(list).findByText("转为 Base64")).toBeInTheDocument();
    expect(within(list).getByText("编码")).toBeInTheDocument();
    expect(within(list).getByText("摘要")).toBeInTheDocument();
    expect(within(list).queryByText("base64Encode")).toBeNull();
  });

  it("切到英文后清单立刻跟着换（不需要重启）", async () => {
    localStorage.setItem("app.settings.locale", "en");
    renderWorkbench();
    await openCatalogue("Expand all");
    const list = await screen.findByLabelText("Algorithm catalogue");
    expect(await within(list).findByText("To Base64")).toBeInTheDocument();
    expect(within(list).getByText("Encoding")).toBeInTheDocument();
  });

  it("切到俄语与日语时分类和条目同样本地化", async () => {
    for (const [locale, label, category, name, expand] of [
      ["ru", "Список алгоритмов", "Кодирование", "В Base64", "Развернуть все"],
      ["ja", "アルゴリズム一覧", "エンコード", "Base64 に変換", "すべて展開"],
    ] as const) {
      localStorage.clear();
      localStorage.setItem("app.settings.locale", locale);
      const view = renderWorkbench();
      const list = await screen.findByLabelText(label);
      await openCatalogue(expand);
      expect(await within(list).findByText(name)).toBeInTheDocument();
      expect(within(list).getByText(category)).toBeInTheDocument();
      view.unmount();
    }
  });

  it("参数名跟着语言走（AES 的「工作模式」在英文里是 Mode）", async () => {
    renderWorkbench();
    await openCatalogue();
    const b64 = await within(catalogue()).findByRole("button", { name: /转为 Base64/ });
    fireEvent.click(b64);
    fireEvent.click(screen.getByRole("button", { name: "转为 Base64" }));
    expect(await screen.findByText("URL 安全字符")).toBeInTheDocument();

    localStorage.setItem("app.settings.locale", "en");
    const second = renderWorkbench();
    await openCatalogue("Expand all");
    const item = await within(screen.getByLabelText("Algorithm catalogue")).findByRole("button", {
      name: /To Base64/,
    });
    fireEvent.click(item);
    fireEvent.click(screen.getByRole("button", { name: "To Base64" }));
    expect(await second.findByText("URL safe")).toBeInTheDocument();
  });

  it("用中文关键词搜算法，也能用算法 id 搜", async () => {
    renderWorkbench();
    await screen.findByLabelText("算法清单");
    const search = screen.getByLabelText("算法清单搜索框");
    fireEvent.change(search, { target: { value: "工作模式" } });
    expect(await within(catalogue()).findByText("AES 加密")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "base64encode" } });
    expect(within(catalogue()).queryByText("转为 Base64")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "不存在的东西" } });
    expect(await screen.findByText("没有匹配的算法")).toBeInTheDocument();
  });
});

describe("配方编排", () => {
  it("点条目加入组合，参数用注册表默认值发给后端", async () => {
    renderWorkbench();
    await openCatalogue();
    const item = await within(catalogue()).findByRole("button", { name: /转为 Base64/ });
    fireEvent.click(item);
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(stepRows()[0]).toHaveTextContent("转为 Base64");
    await waitFor(() => expect(chef.bake).toHaveBeenCalled());
    expect(chef.bake.mock.calls[0][0]).toMatchObject({
      inputEncoding: "utf8",
      recipe: [{ id: "base64Encode", args: { urlSafe: "false", noWrap: "true" } }],
    });
  });

  it("拖拽条目本体到某一步上，会插在那一步之前", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    fireEvent.click(within(catalogue()).getByRole("button", { name: /MD5 摘要/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(2));
    stubRecipeGeometry();

    // 拖 ROT13 落在第二步上半部（该行 top=100、中线=125）= 插在第 2 步之前
    const handle = within(catalogue()).getByText("ROT13");
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 200, clientY: 110 });
    expect(await screen.findByText("放在这里插入")).toBeInTheDocument();
    fireEvent.pointerUp(window, { clientX: 200, clientY: 110 });

    await waitFor(() => expect(stepRows()).toHaveLength(3));
    expect(stepRows()[0]).toHaveTextContent("Base64");
    expect(stepRows()[1]).toHaveTextContent("ROT13");
    expect(stepRows()[2]).toHaveTextContent("MD5");
  });

  it("在配方区外面松手 = 取消，不会莫名多一步", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    stubRecipeGeometry();

    const handle = within(catalogue()).getByText("ROT13");
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 }); // 过阈值，进入拖拽态
    fireEvent.pointerMove(window, { clientX: 200, clientY: 60 }); // 这一步才计算落点
    await waitFor(() => expect(screen.getByText("放在这里插入")).toBeInTheDocument());
    // 拖出面板：插入槽该消失（矩形 x 超过 420 就不在接受区里）
    fireEvent.pointerMove(window, { clientX: 520, clientY: 60 });
    await waitFor(() => expect(screen.queryByText("放在这里插入")).toBeNull());
    fireEvent.pointerUp(window, { clientX: 520, clientY: 60 });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(stepRows()).toHaveLength(1);
  });

  it("Escape 取消拖拽", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    stubRecipeGeometry();

    const handle = within(catalogue()).getByText("ROT13");
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 200, clientY: 60 });
    await screen.findByText("放在这里插入");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByText("放在这里插入")).toBeNull());
    fireEvent.pointerUp(window, { clientX: 200, clientY: 60 });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(stepRows()).toHaveLength(1);
  });

  it("拖拽期间整棵文档禁选、松手后恢复（否则选区锚点会漂到配方面板上）", async () => {
    renderWorkbench();
    await openCatalogue();
    stubRecipeGeometry();
    const root = document.documentElement;
    const handle = within(catalogue()).getByText("ROT13");

    // 起手前先留一段旧选区：拖拽必须把它清掉，否则浏览器会改成"拖这段文字"
    const range = document.createRange();
    range.selectNodeContents(screen.getByTestId("chef-pane-recipe"));
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);

    // 只点一下（没超过阈值）：按下时禁选，抬手立刻恢复成可选
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    expect(window.getSelection()?.rangeCount ?? 0).toBe(0);
    expect(root.classList.contains("chef-dragging")).toBe(true);
    fireEvent.pointerUp(window, { clientX: 20, clientY: 400 });
    expect(root.classList.contains("chef-dragging")).toBe(false);

    // 真拖一次：整个过程一直禁选，落进面板后才恢复
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 });
    expect(root.classList.contains("chef-dragging")).toBe(true);
    fireEvent.pointerMove(window, { clientX: 200, clientY: 300 });
    fireEvent.pointerUp(window, { clientX: 200, clientY: 300 });
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(root.classList.contains("chef-dragging")).toBe(false);
  });

  it("空配方也能拖入（这时落点只能是第 1 步）", async () => {
    renderWorkbench();
    await openCatalogue();
    expect(stepRows()).toHaveLength(0);
    stubRecipeGeometry();

    const handle = within(catalogue()).getByText("ROT13");
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 200, clientY: 300 });
    // 接受态是画在空状态上的（描边 + 淡底），没有行可以插
    expect(screen.getByTestId("chef-recipe-area")).toHaveAttribute("data-picking", "rot13");
    fireEvent.pointerUp(window, { clientX: 200, clientY: 300 });
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(stepRows()[0]).toHaveTextContent("ROT13");
  });

  it("点条目本体就进配方；点行尾加号也进，而且一次点击只加一步", async () => {
    renderWorkbench();
    await openCatalogue();
    const label = await within(catalogue()).findByText("转为 Base64");
    fireEvent.click(label);
    await waitFor(() => expect(stepRows()).toHaveLength(1));

    // 加号在条目内部：不挡住冒泡的话一次点击会加两步，这里验它只加一步
    const md5Label = within(catalogue()).getByText("MD5 摘要");
    fireEvent.click(md5Label);
    await waitFor(() => expect(stepRows()).toHaveLength(2));
    expect(stepRows()[1]).toHaveTextContent("MD5");
    fireEvent.click(within(catalogue()).getByRole("button", { name: "添加 MD5 摘要" }));
    await waitFor(() => expect(stepRows()).toHaveLength(3));
  });

  it("拖完松手那一下不会额外多加一步（拖与点共用同一条目）", async () => {
    renderWorkbench();
    await openCatalogue();
    stubRecipeGeometry();

    const handle = within(catalogue()).getByText("ROT13");
    fireEvent.pointerDown(handle, { button: 0, clientX: 20, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 140, clientY: 400 });
    fireEvent.pointerMove(window, { clientX: 200, clientY: 300 });
    fireEvent.pointerUp(window, { clientX: 200, clientY: 300 });
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    // 真实浏览器里 pointerup 之后还会补一次 click，这里手动补上，验证它被吞掉
    fireEvent.click(handle);
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(stepRows()).toHaveLength(1);
  });


  it("改参数会重新执行并把新值发出去", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /MD5 摘要/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    const step = stepRows()[0];
    fireEvent.click(within(step).getByRole("button", { name: "MD5 摘要" }));
    const checkbox = await within(step).findByRole("checkbox", { name: "大写输出" });
    fireEvent.click(checkbox);
    await waitFor(() =>
      expect(chef.bake.mock.calls.at(-1)?.[0]).toMatchObject({
        recipe: [{ id: "md5", args: { uppercase: "true" } }],
      }),
    );
  });

  it("上移/下移、停用、删除都改变得到的配方", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    fireEvent.click(within(catalogue()).getByRole("button", { name: /MD5 摘要/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(2));

    fireEvent.click(within(stepRows()[1]).getByRole("button", { name: "上移" }));
    await waitFor(() => expect(stepRows()[0]).toHaveTextContent("MD5 摘要"));

    fireEvent.click(within(stepRows()[0]).getByRole("button", { name: "停用该步骤" }));
    await waitFor(() =>
      expect(chef.bake.mock.calls.at(-1)?.[0]).toMatchObject({
        recipe: [{ id: "base64Encode", args: { urlSafe: "false", noWrap: "true" } }],
      }),
    );

    fireEvent.click(within(stepRows()[0]).getByRole("button", { name: "移除该步骤" }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(stepRows()[0]).toHaveTextContent("转为 Base64");
  });

  it("复制/粘贴配方走剪贴板，未知算法会被忽略并报数量", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const readText = vi.fn().mockResolvedValue('[{"id":"md5"},{"id":"nope"}]');
    Object.defineProperty(window.navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText },
    });
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /MD5 摘要/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "复制配方" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(String(writeText.mock.calls[0][0])).toContain("md5");

    fireEvent.click(screen.getByRole("button", { name: "粘贴导入配方" }));
    // 状态条是常驻节点（见 chefUiGuardrails），所以等文案出现而不是等节点出现
    expect(await screen.findByText("已导入配方，忽略 1 个未知算法")).toBeInTheDocument();
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(stepRows()[0]).toHaveTextContent("MD5 摘要");
  });
});

describe("输入输出", () => {
  it("输入文本、输出显示与字节数都来自后端结果", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    fireEvent.change(screen.getByLabelText("输入"), { target: { value: "Hello" } });
    // pre 现在是常驻节点：等内容，不等节点出现
    await waitFor(() => expect(screen.getByTestId("chef-output")).toHaveTextContent("SGVsbG8="));
    expect(screen.getByText("输出 8 字节")).toBeInTheDocument();
    expect(screen.getByText("5 字节")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "已复制" })).toBeInTheDocument());
  });

  it("切到 hex 展示不重跑配方（显示口径不是算法）", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    // pre 是常驻节点：先等文本口径的结果到达，再把调用数当基准
    await waitFor(() => expect(screen.getByTestId("chef-output")).toHaveTextContent("SGVsbG8="));
    const calls = chef.bake.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "十六进制" }));
    await waitFor(() => expect(screen.getByTestId("chef-output")).toHaveTextContent("53 47 56"));
    expect(chef.bake.mock.calls.length).toBe(calls);
  });

  it("某一步失败时该行标红并把后端原因显示出来", async () => {
    chef.bake.mockImplementation((request: ChefBakeRequest) =>
      Promise.resolve(
        resultFor(request, {
          ok: false,
          output: "",
          failedAt: 0,
          steps: [
            { id: "base64Encode", ok: false, bytes: 0, utf8: false, preview: "", error: "Base64 还原失败：长度不是 4 的倍数" },
          ],
        }),
      ),
    );
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /Base64 还原/ }));
    fireEvent.change(screen.getByLabelText("输入"), { target: { value: "abc" } });
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    await waitFor(() => expect(stepRows()[0]).toHaveAttribute("data-failed", "true"));
    expect(
      screen
        .getAllByRole("alert")
        .map((node) => node.textContent)
        .join("\n"),
    ).toContain("长度不是 4 的倍数");
    expect(screen.getByText(/已在/)).toBeInTheDocument();
  });

  it("没有匹配语言的算法时不会把键名漏到界面上", async () => {
    renderWorkbench();
    await screen.findByLabelText("算法清单");
    expect(screen.queryByText(/^chef\./)).toBeNull();
    expect(screen.queryByText(/\$\{|\{\w+\}/)).toBeNull();
  });
});

describe("CyberChef 式四区布局", () => {
  it("清单在左、组合居中、输入在输出之上，顺序不可颠倒", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));

    const before = (a: Element, b: Element) =>
      Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    const panel = (testid: string): HTMLElement => screen.getByTestId(testid);
    const recipe = panel("chef-pane-recipe");
    const input = panel("chef-pane-input");
    const output = panel("chef-pane-output");
    expect(before(screen.getByTestId("chef-pane-catalogue"), recipe)).toBe(true);
    expect(before(recipe, input)).toBe(true);
    expect(before(input, output)).toBe(true);
    // 输入/输出共用右半区（上下分栏），而不是各占一栏
    expect(input.parentElement).toBe(output.parentElement);
  });

  it("主 tab 改名算法中心，第一个子 tab 是算法合集", async () => {
    const zh = DICTIONARIES["zh-CN"] as Record<string, string>;
    const crypto = MAIN_TABS.find((entry) => entry.id === "crypto");
    expect(crypto?.labelKey).toBe("nav.crypto");
    expect(zh[crypto?.labelKey ?? ""]).toBe("算法中心");

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nProvider>
          <CryptoPage />
        </I18nProvider>
      </QueryClientProvider>,
    );
    const tabs = await screen.findByRole("tablist");
    expect(
      within(tabs)
        .getAllByRole("tab")
        .map((tab) => tab.textContent),
    ).toEqual(["算法合集", "签名规则"]);
  });
});

describe("执行控制与恢复现场", () => {
  it("关掉自动执行后只能手动点「执行」", async () => {
    renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(chef.bake).toHaveBeenCalled());
    const before = chef.bake.mock.calls.length;

    // 自动/手动是一枚分段开关：点「手动」= 关掉自动执行，选中态由 aria-pressed 表达
    fireEvent.click(screen.getByRole("button", { name: "手动" }));
    fireEvent.change(screen.getByLabelText("输入"), { target: { value: "changed" } });
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(chef.bake.mock.calls.length).toBe(before);

    fireEvent.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(chef.bake.mock.calls.length).toBe(before + 1));
  });

  it("下次打开时配方、输入、显示方式都还在", async () => {
    const first = renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    fireEvent.change(screen.getByLabelText("输入"), { target: { value: "Hello" } });
    await waitFor(() => expect(localStorage.getItem("chef.workbench.v1")).toContain("base64Encode"));
    first.unmount();

    renderWorkbench();
    await openCatalogue();
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(screen.getByLabelText("输入")).toHaveValue("Hello");
  });

  it("保存我的配方后可以一键回填，删除后不再出现", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("抓包解链");
    const first = renderWorkbench();
    await openCatalogue();
    fireEvent.click(await within(catalogue()).findByRole("button", { name: /转为 Base64/ }));
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "保存为我的配方" }));
    expect(await screen.findByRole("button", { name: /^抓包解链/ })).toBeInTheDocument();
    first.unmount();

    renderWorkbench();
    await openCatalogue();
    const chip = await screen.findByRole("button", { name: /^抓包解链/ });
    fireEvent.click(chip);
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "删除「抓包解链」" }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /^抓包解链/ })).toBeNull(),
    );
  });

  it("清单没到位前不会覆盖上次的现场", async () => {
    let release: (value: ChefOperationDto[]) => void = () => undefined;
    chef.operations.mockImplementation(
      () => new Promise<ChefOperationDto[]>((resolve) => (release = resolve)),
    );
    localStorage.setItem(
      "chef.workbench.v1",
      JSON.stringify({
        input: "old",
        inputEncoding: "utf8",
        autoFire: true,
        display: "text",
        steps: [{ id: "md5", args: { uppercase: "true" }, disabled: false }],
      }),
    );
    renderWorkbench();
    expect(screen.getByText("加载算法清单…")).toBeInTheDocument();
    release(operations);
    await waitFor(() => expect(stepRows()).toHaveLength(1));
    expect(screen.getByLabelText("输入")).toHaveValue("old");
  });

  it("清单读取失败时给出可重试的提示，而不是空清单", async () => {
    chef.operations.mockRejectedValue(new Error("IPC 通道未注册"));
    renderWorkbench();
    expect(await screen.findByText("读取结果失败")).toBeInTheDocument();
  });
});
