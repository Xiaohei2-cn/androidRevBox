/**
 * 会话表的测试（FT1）。
 *
 * 这里测的是**表的行为**：几个会话、谁活着、什么时候才允许杀进程。
 * 用假终端实例与假传输层是有意的——xterm 缓冲区本身的语义由 `terminal.test.ts` 用真 xterm 盯着，
 * 分页这条验收项（"第 2 个在跑，切到第 1 个再回来，输出没丢"）由页面测试盯；
 * 中间这层要盯的是"新开不动旧的 / 上限不静默杀 / 没确认不杀进程"这三件不可逆的事。
 */
import { describe, expect, it, vi } from "vitest";

import type { PtyExit, PtyTransport } from "@/api/pty";
import { createTerminal, type TerminalBundle } from "./terminal";
import { TerminalSessionStore, type TerminalTab } from "./terminalStore";

interface FakeBundle {
  open: ReturnType<typeof vi.fn>;
  focus: ReturnType<typeof vi.fn>;
  disposeTerm: ReturnType<typeof vi.fn>;
  write: ReturnType<typeof vi.fn>;
  bundle: TerminalBundle;
}

function fakeBundle(cols: number, rows: number): FakeBundle {
  const open = vi.fn();
  const focus = vi.fn();
  const disposeTerm = vi.fn();
  const write = vi.fn();
  const term = {
    cols,
    rows,
    element: undefined,
    open,
    focus,
    write,
    getSelection: () => "",
    paste: () => undefined,
    resize: vi.fn(),
    dispose: disposeTerm,
    onData: () => ({ dispose: () => undefined }),
    onTitleChange: () => ({ dispose: () => undefined }),
  } as unknown as TerminalBundle["term"];
  const bundle = {
    term,
    fit: { proposeDimensions: () => undefined },
    serialize: {},
    search: { findNext: vi.fn(() => true), findPrevious: vi.fn(() => true), onDidChangeResults: () => ({ dispose: () => undefined }) },
    renderer: "dom",
    dispose: vi.fn(),
  } as unknown as TerminalBundle;
  return { open, focus, disposeTerm, write, bundle };
}

function fakeTransport(options: { failOpenFor?: string } = {}) {
  const feeders = new Map<string, (bytes: Uint8Array) => void>();
  const exits: Array<(exit: PtyExit) => void> = [];
  let seq = 0;
  const transport: PtyTransport = {
    open: vi.fn(async (args) => {
      seq += 1;
      const id = `pty-${seq}`;
      if (options.failOpenFor === args.program) throw new Error(`起不来：${args.program}`);
      feeders.set(id, args.onData);
      return { id, pid: 1000 + seq, cols: args.cols, rows: args.rows, term: "xterm-256color" };
    }),
    write: vi.fn(async () => undefined),
    resize: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
    defaultProgram: vi.fn(async () => "/bin/zsh"),
    onExit: vi.fn(async (handler) => {
      exits.push(handler);
      return () => undefined;
    }),
  };
  const emitExit = (id: string, exitCode: number | null) => {
    exits.forEach((handler) => handler({ id, exitCode, signal: null }));
  };
  return { transport, feeders, emitExit };
}

/** 建表 + 开一个会话并挂上 DOM：这是"用户点了一次新建终端"的最小完整形状。 */
async function opened(args: { program?: string; max?: number; failOpenFor?: string } = {}) {
  const api = fakeTransport({ failOpenFor: args.failOpenFor });
  const fakes: FakeBundle[] = [];
  const snapshots: TerminalTab[][] = [];
  const store = new TerminalSessionStore({
    transport: api.transport,
    onChange: () => snapshots.push(store.tabs()),
    makeBundle: (options) => {
      const fake = fakeBundle(options.cols, options.rows);
      fakes.push(fake);
      return fake.bundle;
    },
    maxSessions: args.max,
  });
  const created = await store.create({ program: args.program ?? "/bin/zsh", args: [] });
  const host = document.createElement("div");
  const id = created.ok ? created.id : "";
  store.mount(id, host);
  await Promise.resolve();
  await Promise.resolve();
  return { store, api, fakes, snapshots, id, created, host };
}

describe("终端会话表（FT1）", () => {
  /** 分页这条验收的地基：新开一个**不动**之前的，谁都没被杀。 */
  it("新开一个终端不会关掉之前那个", async () => {
    const first = await opened();
    const { store, api } = first;
    const second = await store.create({ program: "/bin/sh", args: [] });
    store.mount(second.ok ? second.id : "", document.createElement("div"));
    const third = await store.create({ program: "top", args: [] });
    store.mount(third.ok ? third.id : "", document.createElement("div"));
    await Promise.resolve();

    expect(store.count()).toBe(3);
    expect(api.transport.close).not.toHaveBeenCalled();
    // 标题不存句子（五语会分叉）：默认只有序号，界面按语言渲染「会话 N」
    expect(store.tabs().map((tab) => tab.title)).toEqual([null, null, null]);
    expect(store.tabs().map((tab) => tab.index)).toEqual([1, 2, 3]);
    // 三条各自一条真会话，id 不重复
    const ptyIds = store.tabs().map((tab) => tab.ptyId);
    expect(new Set(ptyIds).size).toBe(3);
  });

  /** 上限的口径：**拒绝 + 说原因**。悄悄关掉最早那个 = 杀掉用户正在跑的进程。 */
  it("到上限是拒绝新开，不是静默杀一个", async () => {
    const { store, api } = await opened({ max: 2 });
    await store.create({ program: "/bin/sh", args: [] });
    const third = await store.create({ program: "top", args: [] });
    expect(third).toEqual({ ok: false, reason: "limit", max: 2 });
    expect(store.count()).toBe(2);
    expect(api.transport.close).not.toHaveBeenCalled();
  });

  /** 没挂到 DOM 就起进程 = 尺寸只能瞎猜，程序按错宽度排版（C-05）。 */
  it("没挂上元素之前不起进程，挂上才起", async () => {
    const api = fakeTransport();
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => fakeBundle(options.cols, options.rows).bundle,
    });
    const created = await store.create({ program: "/bin/zsh", args: [] });
    expect(api.transport.open).not.toHaveBeenCalled();
    expect(store.tabs()[0].status).toBe("starting");

    store.mount(created.ok ? created.id : "", document.createElement("div"));
    await Promise.resolve();
    await Promise.resolve();
    expect(api.transport.open).toHaveBeenCalledTimes(1);
    expect(store.tabs()[0].status).toBe("running");
  });

  /**
   * 右键菜单的两条底层能力：**选区取自终端自己**（我们不解析内容），
   * **粘贴必须走 `term.paste()`**——只有它才会按 bracketed paste 模式包起来（C-07），
   * 自己拼字节就等于把多行粘贴变成"连着敲回车执行"。
   */
  it("selection 取终端选区；paste 走 term.paste", async () => {
    const api = fakeTransport();
    const fakes: FakeBundle[] = [];
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => {
        const fake = fakeBundle(options.cols, options.rows);
        fakes.push(fake);
        return fake.bundle;
      },
    });
    const created = await store.create({ program: "/bin/zsh", args: [] });
    const id = created.ok ? created.id : "";
    expect(store.selection(id), "还没挂载时不该有选区").toBe("");
    expect(store.paste(id, "ls\n"), "还没挂载时不该假装粘贴成功").toBe(false);

    store.mount(id, document.createElement("div"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    (fakes[0].bundle.term as unknown as { getSelection: () => string }).getSelection = () => "sign=abc";
    expect(store.selection(id)).toBe("sign=abc");

    const pasted: string[] = [];
    (fakes[0].bundle.term as unknown as { paste: (t: string) => void }).paste = (text: string) => {
      pasted.push(text);
    };
    expect(store.paste(id, "sign=abc")).toBe(true);
    expect(pasted).toEqual(["sign=abc"]);
  });

  /**
   * **这条钉的就是"我这边测试全绿、用户界面上只有一颗灰点"那个 bug。**
   *
   * 真实浏览器里挂载时量尺寸一定成功（jsdom 永远量不到），量到的尺寸与初始 80×24 不同
   * ⇒ 会话会发一次"还没在跑"的状态通知。旧代码把它当成"已结束"，状态翻成 exited，
   * 于是"状态还是 starting 才去起进程"那句永远不成立 ⇒ 进程永不起、不报错、日志里也没有 pty_open。
   * 换句话说：**量到了尺寸这件好事，反而让会话永远起不来。**
   */
  it("挂载时量到了不同尺寸，也照样得起进程（不许翻成已退出）", async () => {
    const api = fakeTransport();
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => {
        const fake = fakeBundle(options.cols, options.rows);
        (fake.bundle as unknown as { fit: { proposeDimensions: () => unknown } }).fit.proposeDimensions = () => ({
          cols: 151,
          rows: 43,
        });
        return fake.bundle;
      },
    });
    const created = await store.create({ program: "/bin/zsh", args: [] });
    store.mount(created.ok ? created.id : "", document.createElement("div"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(api.transport.open).toHaveBeenCalledTimes(1);
    const tab = store.tabs()[0];
    expect(tab.status).not.toBe("exited");
    expect(tab.status).not.toBe("stopped");
    // 量到的尺寸仍然要传下去（C-05 那两头对齐不能被这次修复弄丢）
    expect(tab.cols).toBe(151);
    expect(vi.mocked(api.transport.open).mock.calls[0][0].cols).toBe(151);
  });


  /** StrictMode 会把 ref 回调 detach/attach 各来一遍：多起一条 PTY 就是多一个没人关的进程。 */
  it("同一个 tab 挂两次只开一条 PTY，也只 open 一次", async () => {
    const { store, api, id, fakes, host } = await opened();
    store.mount(id, host);
    store.mount(id, host);
    await Promise.resolve();
    expect(api.transport.open).toHaveBeenCalledTimes(1);
    expect(fakes[0].open).toHaveBeenCalledTimes(1);
    expect(store.tabs()[0].ptyId).toBe("pty-1");
  });

  /** xterm 不 open 就一个字也不画——FT0 的生产路径就是漏了这条（真窗口一片黑）。 */
  it("mount 真的把终端 open 到了宿主元素上", async () => {
    const { fakes, host, id, store, api } = await opened();
    expect(fakes[0].open).toHaveBeenCalledWith(host);
    store.mount(id, null); // React 摘元素（卸载）：不许重开，更不许顺手把会话关掉
    expect(fakes[0].open).toHaveBeenCalledTimes(1);
    expect(api.transport.close).not.toHaveBeenCalled();
    expect(store.count()).toBe(1);
  });

  /** rename：改名要能改到真值，空/纯空白要拒绝（清空的标题比长标题更糟） */
  it("rename 改标题，空名字被拒", async () => {
    const first = await opened();
    const { store, id } = first;
    expect(store.rename(id, "  签名抓取  ")).toBe(true);
    expect(store.tabs()[0].title).toBe("签名抓取");
    expect(store.rename(id, "   ")).toBe(false);
    expect(store.tabs()[0].title).toBe("签名抓取");
    // 不存在的 tab 不该假装成功（vitest 的 toBe 不收第二个参数，提示写在 expect 上）
    expect(store.rename("term-999", "x"), "不存在的 tab 不该假装成功").toBe(false);
  });

  /** 关运行中的会话 = 内核给前台进程组发 SIGHUP，不可逆。没确认什么都不许杀。 */
  it("运行中的 tab 不确认就不杀，确认了只杀它那一个", async () => {
    const first = await opened();
    const { store, api, id } = first;
    const second = await store.create({ program: "/bin/sh", args: [] });
    const secondId = second.ok ? second.id : "";
    store.mount(secondId, document.createElement("div"));
    await Promise.resolve();

    await expect(store.close(id)).resolves.toEqual({ closed: false, needsConfirm: true });
    expect(api.transport.close).not.toHaveBeenCalled();
    expect(store.count()).toBe(2);

    await expect(store.close(id, { force: true })).resolves.toEqual({ closed: true });
    expect(api.transport.close).toHaveBeenCalledTimes(1);
    expect(api.transport.close).toHaveBeenCalledWith("pty-1");
    expect(store.tabs().map((tab) => tab.id)).toEqual([secondId]);
  });

  /** 已退出的会话没什么可确认：直接关，但也不该去 close 一条已经没了的 pty。 */
  it("进程自己退出后不要求确认，close 也只发一次", async () => {
    const { store, api, id } = await opened();
    api.emitExit("pty-1", 3);
    expect(store.tabs()[0].status).toBe("exited");
    expect(store.tabs()[0].exitCode).toBe(3);

    await expect(store.close(id)).resolves.toEqual({ closed: true });
    // 进程已经自己走了，不该再对着一条死 pty 发 close
    expect(api.transport.close).not.toHaveBeenCalled();
    expect(store.count()).toBe(0);
  });

  /** 「停止」与「关闭」是两件事：停止只杀进程，屏上的内容还要能往上滚。 */
  it("停止只杀进程、留着这块屏", async () => {
    const { store, api, id } = await opened();
    await store.stop(id);
    expect(api.transport.close).toHaveBeenCalledWith("pty-1");
    const tab = store.tabs()[0];
    expect(tab.status).toBe("stopped");
    expect(tab.ptyId).toBe(null);
    // 还在表里：屏上内容没被丢
    expect(store.count()).toBe(1);
  });

  /** 起不来只影响它自己那一格，别的会话继续跑。 */
  it("启动失败只标这一个 tab", async () => {
    const api = fakeTransport({ failOpenFor: "boom" });
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => fakeBundle(options.cols, options.rows).bundle,
    });
    const good = await store.create({ program: "/bin/zsh", args: [] });
    store.mount(good.ok ? good.id : "", document.createElement("div"));
    const bad = await store.create({ program: "boom", args: [] });
    store.mount(bad.ok ? bad.id : "", document.createElement("div"));
    await Promise.resolve();
    await Promise.resolve();

    const tabs = store.tabs();
    expect(tabs[0].status).toBe("running");
    expect(tabs[1].status).toBe("failed");
    expect(tabs[1].error).toContain("起不来：boom");
    expect(store.count()).toBe(2);
  });

  /** R-08：离开页面才是批量收摊的地方，且每个活会话各关一次。 */
  it("disposeAll 收掉全部活动会话", async () => {
    const { store, api, id } = await opened();
    const second = await store.create({ program: "/bin/sh", args: [] });
    store.mount(second.ok ? second.id : "", document.createElement("div"));
    await Promise.resolve();
    const third = await store.create({ program: "top", args: [] });
    // 第三条不挂 DOM：没有进程，也就无所谓关
    expect(third.ok).toBe(true);

    store.disposeAll();
    const closed = vi.mocked(api.transport.close).mock.calls.map((call) => call[0]).sort();
    expect(closed).toEqual(["pty-1", "pty-2"]);
    expect(store.tabs()).toEqual([]);
    expect(id).toBeTruthy();
  });

  /** 尺寸两头对齐后才起进程：传 0 进去，程序会以为自己 0 列（C-05 的教训）。 */
  it("起进程时下传的尺寸不是 0，并写回 tab", async () => {
    const { store, api, fakes, id } = await opened();
    expect(api.transport.open).toHaveBeenCalledTimes(1);
    const passed = vi.mocked(api.transport.open).mock.calls[0][0];
    expect(passed.cols).toBeGreaterThan(0);
    expect(passed.rows).toBeGreaterThan(0);
    expect([store.tabs()[0].cols, store.tabs()[0].rows]).toEqual([passed.cols, passed.rows]);
    // 量不到尺寸（这里的假 fit 拿不到建议值）时 fit 必须报 false 且什么都不改
    expect(store.fit(id)).toBe(false);
    expect(api.transport.resize).not.toHaveBeenCalled();
    expect(fakes[0].bundle.term.cols).toBe(80);
  });

  /**
   * 两个会话并存时，键盘字节必须只进自己那条 pty——这是"多 tab 互相不串台"的正面证据。
   * 这里用**真 xterm**（假终端没有 input()，也复现不了 xterm 把按键编码成什么字节）。
   */
  it("在哪个 tab 里打字就只写哪条会话（真 xterm）", async () => {
    const api = fakeTransport();
    const real: TerminalBundle[] = [];
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => {
        const bundle = createTerminal(options);
        real.push(bundle);
        return bundle;
      },
    });
    const first = await store.create({ program: "/bin/zsh", args: [] });
    const second = await store.create({ program: "/bin/sh", args: [] });
    const [id1, id2] = [first.ok ? first.id : "", second.ok ? second.id : ""];
    store.mount(id1, document.createElement("div"));
    store.mount(id2, document.createElement("div"));
    await new Promise((resolve) => setTimeout(resolve, 20));

    (real[0].term as unknown as { input: (data: string) => void }).input("ls\n");
    await new Promise((resolve) => setTimeout(resolve, 20));

    const written = vi.mocked(api.transport.write).mock.calls.map((call) => call[0]);
    expect(written.length).toBeGreaterThan(0);
    expect(new Set(written)).toEqual(new Set([store.tabs()[0].ptyId]));
    expect(store.tabs()[1].ptyId).not.toBe(store.tabs()[0].ptyId);
    expect(id2).toBeTruthy();
    store.disposeAll();
  });

  /**
   * 挂载时抛异常**必须变成界面能看见的失败**。
   *
   * 这条钉的是第八十八轮那一类现场：`term.open()` 已经完成（所以屏上写出了我们盖的那行命令），
   * 紧接着挂载路径上抛了一下 —— 异常顺着 React 的 ref 回调只去控制台，
   * 于是"有字、没进程、没报错、日志里也什么都没有"，谁都没法查。
   */
  it("挂载抛异常时报成失败，而不是安静地什么都不做", async () => {
    const api = fakeTransport();
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => {
        const fake = fakeBundle(options.cols, options.rows);
        (fake.open as ReturnType<typeof vi.fn>).mockImplementation(() => {
          throw new Error("容器尺寸算不出来");
        });
        return fake.bundle;
      },
    });
    const created = await store.create({ program: "/bin/zsh", args: [] });
    store.mount(created.ok ? created.id : "", document.createElement("div"));
    await new Promise((resolve) => setTimeout(resolve, 20));

    const tab = store.tabs()[0];
    expect(tab.status).toBe("failed");
    expect(tab.error).toContain("容器尺寸算不出来");
    // 报错了也不许偷偷起一条"没人看着"的进程
    expect(api.transport.open).not.toHaveBeenCalled();
  });

  /**
   * 已收摊的表必须**如实说"表被收了"**。
   * 它以前返回 `empty-program`，于是界面会在一个填得好好的命令框旁边说"先说要跑什么程序"
   * ——那是把内部状态谎报成用户的输入错误（第八十九轮用户的截图就是这么来的）。
   */
  it("已 disposeAll 的表报 store-disposed，不谎称程序为空", async () => {
    const api = fakeTransport();
    const store = new TerminalSessionStore({
      transport: api.transport,
      onChange: () => undefined,
      makeBundle: (options) => fakeBundle(options.cols, options.rows).bundle,
    });
    store.disposeAll();
    expect(await store.create({ program: "/bin/zsh", args: [] })).toEqual({ ok: false, reason: "store-disposed" });
  });

  /** 快照必须是"值"：调用方拿到内部对象的话，React 的比较与保活就都不算了。 */
  it("tabs() 给的是快照，改不动内部状态", async () => {
    const { store } = await opened();
    const snapshot = store.tabs();
    snapshot[0].status = "failed";
    snapshot.push(snapshot[0]);
    expect(store.count()).toBe(1);
    expect(store.tabs()[0].status).toBe("running");
  });
});
