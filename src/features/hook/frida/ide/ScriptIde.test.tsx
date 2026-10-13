/**
 * ScriptIde 的模型管理（jsdom 里跑不了真 monaco——worker/DOM 一大套，
 * harness 实测归浏览器；这里把 `./monacoSetup` 换成一个能记录调用的假内核，
 * 钉住我们自己的那几条不变量）：
 *
 * 1. **切文件不丢改动**：A 改了一半 → 切 B → 切回 A，A 的正文还是改过的样子，
 *    而且不会因为"重新打开"被磁盘内容覆盖（旧版实现切文件 dispose 模型，
 *    敲了一半的东西直接没了——这是丢数据，不是省内存）；
 * 2. **脏集合按文件记**：A 脏、B 干净时集合是 {A}；保存后 A 从集合里消失；
 * 3. **保存走 scriptWrite(false)**、新建走 scriptWrite(true)（撞名拒绝的语义在宿主侧）；
 * 4. **删除后模型与脏集合都清掉**。
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/i18n/I18nProvider";

// ===== 假 monaco 内核（只实现 ScriptIde 真正用到的那几个成员） =====

interface FakeModel {
  uri: string;
  value: string;
  listeners: Array<() => void>;
  disposed: boolean;
  getValue(): string;
  setValue(v: string): void;
  onDidChangeContent(cb: () => void): { dispose(): void };
  setEOL(): void;
  dispose(): void;
}

const kernels = vi.hoisted(() => {
  const models = new Map<string, unknown>();
  return { models, editor: null as unknown };
});

function makeFakeMonaco() {
  const createModel = (value: string, _lang: string, uri: { toString(): string }) => {
    const m: FakeModel = {
      uri: uri.toString(),
      value,
      listeners: [],
      disposed: false,
      getValue: () => m.value,
      setValue: (v: string) => {
        m.value = v;
        m.listeners.forEach((cb) => cb());
      },
      onDidChangeContent: (cb) => {
        m.listeners.push(cb);
        return { dispose: () => undefined };
      },
      setEOL: () => undefined,
      dispose: () => {
        m.disposed = true;
        kernels.models.delete(m.uri);
      },
    };
    kernels.models.set(m.uri, m);
    return m;
  };

  let current: FakeModel | null = null;
  const editor = {
    setModel: (m: FakeModel) => {
      current = m;
    },
    saveViewState: () => ({ fake: true }),
    restoreViewState: () => undefined,
    // activate() 里有一次尺寸自愈的 layout()（宿主曾不可见时把 5×5 僵值救回来）：
    // 真编辑器有这个方法，假内核也得有，否则每次打开文件都会在 activate 里炸掉
    layout: () => undefined,
    focus: () => undefined,
    dispose: () => undefined,
  };

  /** 每次 create 的宿主（自愈断言：宿主里只许留当前这一个编辑器节点） */
  const createdHosts: HTMLElement[] = [];

  return {
    monaco: {
      Uri: { parse: (s: string) => ({ toString: () => s }) },
      editor: {
        create: (host: HTMLElement) => {
          // 与真 Monaco 同构：create 会往宿主里**追加**一个节点（不会自己清场）
          const node = document.createElement("div");
          node.className = "monaco-editor";
          host.appendChild(node);
          createdHosts.push(host);
          return editor;
        },
        createModel,
        EndOfLineSequence: { LF: 0 },
        defineTheme: () => undefined,
        setTheme: () => undefined,
      },
    },
    getCurrent: () => current,
    hosts: () => createdHosts,
  };
}

const fake = makeFakeMonaco();

const applyMock = vi.hoisted(() => ({
  applyFridaTypes: vi.fn(async (_v: unknown, dir: string) =>
    dir ? `类型库：@types/frida-gum@18.8.2  ← ${dir}` : "类型库：按 frida 版本解析（内置兜底）",
  ),
}));
vi.mock("@/features/hook/frida/ide/monacoSetup", () => ({
  setupMonaco: () => fake.monaco,
  ensureIdeTheme: () => "test-theme",
  applyFridaTypes: applyMock.applyFridaTypes,
}));

const hookApiMock = vi.hoisted(() => ({
  scriptRead: vi.fn(),
  scriptWrite: vi.fn(),
  scriptDelete: vi.fn(),
  fridaTypes: vi.fn(async () => ({
    gum: "// env gum",
    gumVersion: "18.8.2",
    gumHasJava: true,
    javaBridge: null,
    javaBridgeVersion: null,
    gumPath: "/proj/node_modules/@types/frida-gum/index.d.ts",
    javaBridgePath: null,
  })),
  fridaTypesDirs: vi.fn(async () => [
    { projectRoot: "/proj", version: "18.8.2", mtime: 1700000000 },
  ]),
}));

vi.mock("@/api/hook", () => ({ hookApi: hookApiMock }));

// envApi.frida：类型库解析会读它（拿 frida 版本）
const envMock = vi.hoisted(() => ({ frida: vi.fn(async () => ({ fridaVersion: "17.17.0" })) }));
vi.mock("@/api/env", () => ({ envApi: envMock }));

// 空态里的「选择目录」走 workdir.ts（dialog + config），测试里都不该真弹/真写
const dialogMock = vi.hoisted(() => ({
  pickDirectory: vi.fn<() => Promise<string | null>>(async () => null),
}));
vi.mock("@/api/dialog", () => ({ pickDirectory: dialogMock.pickDirectory }));
const configMock = vi.hoisted(() => ({
  set: vi.fn(async () => null),
  get: vi.fn(async () => ""),
}));
vi.mock("@/api/config", () => ({
  configApi: { set: configMock.set, get: configMock.get },
}));

// useSettings 需要 AppProviders；这里给个最小 provider 替身
vi.mock("@/app/providers", () => ({ useSettings: () => ({ effectiveTheme: "dark" }) }));

import { ScriptIde } from "./ScriptIde";

const FILES: Record<string, string> = {
  "a.js": "send('a');\n",
  "b.js": "send('b');\n",
};

/** 让外部请求打开某个文件（nonce 递增） */
function openRequest(name: string, nonce: number) {
  return { name, nonce };
}

beforeEach(() => {
  kernels.models.clear();
  hookApiMock.scriptRead.mockReset().mockImplementation(async (name: string) => FILES[name] ?? "");
  hookApiMock.scriptWrite.mockReset().mockResolvedValue(12);
  hookApiMock.scriptDelete.mockReset().mockResolvedValue(undefined);
});

describe("空态：欢迎面板（顶部对齐，一条可点的下一步）", () => {
  it("有工作目录但没打开脚本：标题 + 「新建」按钮 + 当前目录 + 三条上手提示", () => {
    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={null} onRun={vi.fn()} />
      </I18nProvider>,
    );
    const panel = screen.getByTestId("ide-welcome");
    expect(panel.textContent).toContain("还没有打开脚本");
    expect(screen.getByTestId("ide-welcome-new")).toBeTruthy();
    expect(panel.textContent).toContain("/w");
    // 三条提示都在（智能提示那条是"这个地方能提示 frida 代码"的说明）
    expect(panel.textContent).toContain("Interceptor.");
    // 顶部对齐：面板是内容从顶部走的容器（flex-col 且不是居中）
    expect(panel.className).not.toContain("items-center");
    expect(panel.className).not.toContain("justify-center");
  });

  it("没有工作目录：「选择目录」CTA 在，点它会弹选择器并把新目录回报出去", async () => {
    const onWorkdirChange = vi.fn();
    dialogMock.pickDirectory.mockResolvedValueOnce("/picked/hooks");
    render(
      <I18nProvider>
        <ScriptIde workdir="" request={null} onRun={vi.fn()} onWorkdirChange={onWorkdirChange} />
      </I18nProvider>,
    );
    const panel = screen.getByTestId("ide-welcome");
    expect(panel.textContent).toContain("先选好脚本工作目录");

    act(() => screen.getByTestId("ide-welcome-pick").click());
    await waitFor(() => expect(configMock.set).toHaveBeenCalledWith("app.hook.workdir", "/picked/hooks"));
    expect(onWorkdirChange).toHaveBeenCalledWith("/picked/hooks");
  });
});

describe("编辑器单例与宿主自愈（双实例事故的回归）", () => {
  it("宿主里先塞一个孤儿编辑器节点，ensureEditor 会清掉它再建——屏上只留一个", async () => {
    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 1)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('a');\n"));
    const host = fake.hosts().at(-1);
    expect(host).toBeTruthy();
    // 真实路径：宿主里只有我们建的这一个（孤儿在 create 前被 replaceChildren 清掉）
    expect(host!.querySelectorAll(".monaco-editor").length).toBe(1);
  });

  it("先手动塞两个孤儿节点（模拟历史 HMR 竞态残留），重挂后宿主里只剩 1 个", async () => {
    const view = render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 1)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('a');\n"));
    const host = fake.hosts().at(-1)!;
    // 模拟"上面一大片空白"的现场：宿主里叠着没人管的空壳
    for (let i = 0; i < 2; i++) {
      const orphan = document.createElement("div");
      orphan.className = "monaco-editor";
      host.appendChild(orphan);
    }
    expect(host.querySelectorAll(".monaco-editor").length).toBe(3);

    // **真正的卸载 → 再挂载**（StrictMode / HMR 的形状；rerender 同一实例测不出这个）：
    // 新挂载必须重新建编辑器，并且创建前的宿主自愈把孤儿清干净
    view.unmount();
    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 2)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => {
      const h = fake.hosts().at(-1);
      expect(h).toBeTruthy();
      expect(h!.querySelectorAll(".monaco-editor").length).toBe(1);
    });
  });
});

describe("类型库来源：单一可选读取失败不许拖垮整条链路", () => {
  /**
   * 事故现场（真实）：原来写成 `await Promise.all([envApi.frida(), configApi.get(KEY,"")])`，
   * 而 `configApi.get` 对**未注册的配置键**会抛错（Rust 白名单校验）——于是"读一个可选
   * 配置项失败"把整条链路带崩，类型库根本没换成用户那份，而面板上看不出任何异常
   * （只有提示内容不对）。这条钉住：即使读配置抛错，自动发现仍必须继续走到底。
   */
  it("configApi.get 抛错时，仍会走自动发现并注入项目那份类型库", async () => {
    configMock.get.mockRejectedValueOnce(new Error("未注册的配置键: app.hook.frida_types_dir"));
    hookApiMock.fridaTypesDirs.mockClear();
    applyMock.applyFridaTypes.mockClear();

    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={null} onRun={vi.fn()} />
      </I18nProvider>,
    );

    // 自动发现必须仍被调用（说明没被前面那个异常短路）
    await waitFor(() => expect(hookApiMock.fridaTypesDirs).toHaveBeenCalled());
    // 关键：最终交给注入的目录是**发现到的那份**（而不是空 → 回退内置）
    await waitFor(() => expect(applyMock.applyFridaTypes).toHaveBeenCalledWith("17.17.0", "/proj"));
    await waitFor(() => {
      const note = screen.queryByTestId("ide-types-note");
      expect(note?.textContent ?? "").toContain("18.8.2");
    });
  });

  it("自动发现也失败（旧后端无此命令）时，回退按 frida 版本解析——不静默用内置", async () => {
    configMock.get.mockResolvedValue("");
    hookApiMock.fridaTypesDirs.mockRejectedValueOnce(new Error("unknown command"));
    applyMock.applyFridaTypes.mockClear();

    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={null} onRun={vi.fn()} />
      </I18nProvider>,
    );

    // 发现失败 → 目录为空，交给注入的是空串（由它走"按 frida 版本解析"）
    await waitFor(() => expect(applyMock.applyFridaTypes).toHaveBeenCalledWith("17.17.0", ""));
    // 仍要有结论（回退说明），而不是空着
    await waitFor(() => {
      const note = screen.queryByTestId("ide-types-note");
      expect(note?.textContent ?? "").toBeTruthy();
    });
  });
});

describe("ScriptIde 的模型管理", () => {
  it("切文件不丢改动：A 改一半 → 切 B → 切回 A，改动还在", async () => {
    const { rerender } = render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 1)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('a');\n"));

    // 在 A 里改一半（不保存）
    act(() => fake.getCurrent()?.setValue("send('a edited');\n"));

    // 切到 B
    rerender(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("b.js", 2)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('b');\n"));

    // 切回 A：改动仍在（而不是被磁盘内容覆盖）
    rerender(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 3)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('a edited');\n"));
    // 且没有因为切回来重新读盘（脏文件的磁盘版本不刷新）
    expect(hookApiMock.scriptRead).toHaveBeenCalledTimes(2); // a 一次、b 一次
  });

  it("脏集合按文件记：a 脏时集合含 a；保存后清掉", async () => {
    const onDirtyChange = vi.fn();
    render(
      <I18nProvider>
        <ScriptIde
          workdir="/w"
          request={openRequest("a.js", 1)}
          onRun={vi.fn()}
          onDirtyChange={onDirtyChange}
        />
      </I18nProvider>,
    );
    await waitFor(() => expect(fake.getCurrent()?.value).toBe("send('a');\n"));

    act(() => fake.getCurrent()?.setValue("dirty\n"));
    await waitFor(() => {
      const last = onDirtyChange.mock.calls.at(-1)?.[0] as ReadonlySet<string> | undefined;
      expect(last?.has("a.js")).toBe(true);
    });

    // 保存（点工具条上的保存按钮）：scriptWrite(false)，集合清掉 a
    act(() => screen.getByTestId("ide-save").click());
    await waitFor(() => expect(hookApiMock.scriptWrite).toHaveBeenCalledWith("a.js", "dirty\n", false));
    await waitFor(() => {
      const last = onDirtyChange.mock.calls.at(-1)?.[0] as ReadonlySet<string> | undefined;
      expect(last?.has("a.js")).toBe(false);
    });
  });

  it("删除：模型被 dispose、当前文件清空", async () => {
    render(
      <I18nProvider>
        <ScriptIde workdir="/w" request={openRequest("a.js", 1)} onRun={vi.fn()} />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("ide-current-file").textContent).toContain("a.js"));

    // 初次删除会进入二次确认
    act(() => screen.getByTestId("ide-delete").click());
    await waitFor(() => expect(screen.getByTestId("ide-delete-confirm")).toBeTruthy());
    act(() => screen.getByTestId("ide-delete-confirm").click());

    await waitFor(() => expect(hookApiMock.scriptDelete).toHaveBeenCalledWith("a.js"));
    await waitFor(() =>
      expect(screen.getByTestId("ide-current-file").textContent).toContain("未打开脚本"),
    );
    // 模型被放掉（假内核里 dispose 会把自己从表里删掉）
    await waitFor(() => expect(kernels.models.size).toBe(0));
  });
});