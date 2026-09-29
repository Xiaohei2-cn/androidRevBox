/**
 * 终端会话表（FT1）：**新开一个终端不关掉之前那个**。
 *
 * 这一层存在的理由只有一个：把"有几个会话、每个会话什么状态、什么时候该被杀掉"
 * 从 React 组件里搬出来，让它能脱离 WebView 被逐条测（分页这条验收项要求的正是
 * "第 2 个在跑、切到第 1 个再切回来，输出没丢"——那是状态表的行为，不是渲染的行为）。
 *
 * 三条边界写在这儿，免得后人当成"顺手能改"：
 * 1. **上限不静默杀**：到 8 个就拒绝新开并说明原因。悄悄关掉最早那个 = 杀掉用户正在跑的进程。
 * 2. **运行中的 tab 必须确认才关**：关 tab 会关掉 PTY，内核随即给前台进程组发 SIGHUP，
 *    那是不可逆的。已退出的 tab 没什么可确认的，直接关。
 * 3. **组件卸载 ≠ 关会话**：隐藏 tab 的 host 元素被 React 摘掉时不碰会话（保活）；
 *    只有页面整体卸载（`disposeAll`）才收掉全部——那才是 R-08 的"离开页面不留活进程"。
 */
import type { PtyTransport } from "@/api/pty";
import { createTerminal, type TerminalBundle } from "./terminal";
import { TerminalSession } from "./terminalSession";

/** 同时最多几个活动会话（§4.4）。到顶就提示，不静默杀。 */
export const MAX_TERMINAL_SESSIONS = 8;

export type TerminalTabStatus = "running" | "exited" | "stopped" | "starting" | "failed";

export interface TerminalTab {
  id: string;
  /** 宿主：H-1 本机 PTY(`local`) / H-2 设备 shell(`device`) / 官方 frida REPL(`frida`) */
  kind: "local" | "device" | "frida";
  /** 用户改过的名字；**没改过就是 null**，界面按序号显示「会话 N」
   * （本地化文案归词典，表里不存句子——否则五语就得分叉） */
  title: string | null;
  /** 第几个会话（从 1 开始），界面上"会话 1 / 会话 2"就是它 */
  index: number;
  /** 这块屏跑的是什么 + 参数：悬停与菜单"复制当前路径"都要用 */
  program: string;
  args: string[];
  ptyId: string | null;
  status: TerminalTabStatus;
  exitCode: number | null;
  /** 没有退出码时的信号名（`null` = 不知道/没给）。见 `syncFromSession` 里那条区分 */
  exitSignal: string | null;
  error: string | null;
  cols: number;
  rows: number;
  renderer: "webgl" | "dom";
}

export interface CreateTabInput {
  program: string;
  args: string[];
  cwd?: string | null;
  /** 缺省 `local`；设备/frida 会话必须显式写（状态灯旁那句"在跑 80×24"对不同宿主含义不同） */
  kind?: "local" | "device" | "frida";
  /** 显式标题（设备会话用 `adb shell @ <serial>`）；不给就取程序名 + 序号 */
  title?: string;
}

/**
 * 结果里**只带机器码，不带界面文案**：这层在 `src/lib`，文案归五语词典
 * （`hook.terminal.*`）。这里写死中文就等于让 en/ja/pt-BR/ru 看见一句汉语。
 */
export type CreateTabResult =
  | { ok: true; id: string }
  | { ok: false; reason: "limit"; max: number }
  | { ok: false; reason: "empty-program" }
  /** 这张表已经被收过（卸载/StrictMode 假卸载）：**必须与"程序为空"分开报**，
   * 否则界面会对着一张填得好好的命令框说"先说要跑什么程序"。 */
  | { ok: false; reason: "store-disposed" }
  | { ok: false; reason: "start-failed"; message: string };

export type CloseResult =
  | { closed: false; needsConfirm: true }
  | { closed: true };

interface Entry {
  tab: TerminalTab;
  session: TerminalSession;
  bundle: TerminalBundle;
  /** `begin()` 只许进一次：StrictMode 会把 ref 回调 detach/attach 各来一遍，
   * 没有这道闸就会真的开出两条 PTY（第二条没人关，就是个活进程） */
  beginning: boolean;
  /** **我们自己**调过 stop()：这是"已停止"与"死了但没退出码"唯一的分别 */
  stopRequested: boolean;
  args: string[];
  cwd: string | null;
}

export interface StoreDeps {
  transport: PtyTransport;
  /** 变更通知：React 用它把快照刷进 state。测试里它就是断言点。 */
  onChange: () => void;
  /** 造终端实例——测试注入，生产用 `createTerminal`。 */
  makeBundle?: (options: { cols: number; rows: number }) => TerminalBundle;
  maxSessions?: number;
}

/** 起不到会话时给界面的兜底尺寸（jsdom 量不到、隐藏容器也量不到，绝不能传 0）。 */
const FALLBACK_COLS = 80;
const FALLBACK_ROWS = 24;

export class TerminalSessionStore {
  private entries: Entry[] = [];

  private disposed = false;

  private readonly maxSessions: number;

  /**
   * 序号**跟着表走**，不用模块级全局：全局计数器让 id 跨页面/跨测试一直往后涨
   * （`term-7`、`term-8`……），于是"关掉第一个 tab"这种断言写成 `term-1` 就会红，
   * 而红的原因和代码对不对毫无关系。id 只要在一张表里唯一就够用。
   */
  private seq = 0;

  constructor(private readonly deps: StoreDeps) {
    this.maxSessions = deps.maxSessions ?? MAX_TERMINAL_SESSIONS;
  }

  /** 快照只给"值"，不给内部对象：调用方拿到的是可以直接渲染的普通数据。 */
  tabs(): TerminalTab[] {
    return this.entries.map((entry) => ({ ...entry.tab }));
  }

  /** 表是不是已经被 disposeAll() 收掉：面板靠它决定"要不要重建一张新表"（见 TerminalPanel 的 ensureStore）。 */
  get isDisposed(): boolean {
    return this.disposed;
  }

  count(): number {
    return this.entries.length;
  }

  bundle(id: string): TerminalBundle | null {
    return this.entries.find((entry) => entry.tab.id === id)?.bundle ?? null;
  }

  /**
   * 把某个 tab 的终端挂到 host 上。**生产路径必须走这里**：
   * xterm 只有 `open(element)` 之后才真的往 DOM 里画东西——FT0 那版页面只在测试里 open 过，
   * 真窗口就是一块黑。这条腿现在由 `TerminalPage` 的 ref 回调调用，并有测试钉着（见 FT1 执行记录）。
   */
  mount(id: string, host: HTMLElement | null): void {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry || !host) {
      // React 摘元素（tab 隐藏不会走到这儿，只有卸载会）：**不碰会话**，保活是 §4.4 的硬要求
      return;
    }
    // "挂没挂过"只有会话自己知道（`session.mount` 里那一个守卫），这里不再记第二份
    try {
      entry.session.mount(host);
    } catch (error) {
      // 挂载抛异常也**不能吞**：以前它会顺着 React 的 ref 回调跑到控制台，
      // 于是屏上有字（open 成功了）、进程却没起、界面上一句错误都没有
      this.onSessionError(entry, `挂载终端时异常：${String((error as Error)?.message ?? error)}`);
    }
    // 挂上才量得到尺寸；量完这一趟，才轮到把进程起来。
    // 放在 try 外面：挂载失败时上面已经把原因报出去了，这里若状态已不是 starting 会自己跳过。
    if (entry.tab.status === "starting") {
      // **不能 `void`**：不等待的 promise 一旦 reject，就是没人接的异常 ——
      // 用户侧表现为"点了没反应"。这里显式收下并写进屏里。
      this.begin(entry).catch((error) => {
        this.onSessionError(entry, `起会话时异常：${String((error as Error)?.message ?? error)}`);
      });
    }
  }

  /** 界面尺寸变了/重新可见了：量一次，量不到就什么都不改（R-03）。 */
  fit(id: string): boolean {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry?.session.mounted) return false;
    return entry.session.fit();
  }

  /** 选区文本（菜单"复制"用）。没有选区返回空串，由调用方去说"没选中东西" */
  selection(id: string): string {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry?.session.mounted) return "";
    try {
      return entry.bundle.term.getSelection();
    } catch {
      return "";
    }
  }

  /**
   * 粘贴。**必须走 `term.paste()`** 而不是直接写字节：
   * 那样 xterm 才会按程序的 bracketed paste 模式（C-07）包上 ESC[200~/201~，
   * 多行粘贴也不会被当成"连按回车执行"。
   */
  paste(id: string, text: string): boolean {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry?.session.mounted) return false;
    entry.bundle.term.paste(text);
    return true;
  }

  focus(id: string): void {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (entry?.session.mounted) entry.bundle.term.focus();
  }

  /**
   * 重新可见时补一次重绘（§4.4「重新可见必须 fit + notifyResize」的后半）。
   *
   * 这条不是保险，是补一个真会漏的洞：字节在 tab 隐藏期间照样进缓冲区（会话一直活着），
   * 但渲染器在 `display:none` 的容器里不刷新——切回来的那一瞬间屏上还是旧内容，
   * 用户看到的就是"输出丢了"。尺寸没变时 `fit()` 什么都不做，所以必须显式刷一次。
   */
  repaint(id: string): void {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry?.session.mounted) return;
    const rows = Math.max(0, entry.bundle.term.rows - 1);
    entry.bundle.term.refresh(0, rows);
  }

  /**
   * 改 tab 标题（右键菜单里的"重命名"）。
   * 空串与纯空白一律拒绝（清空后这块屏就没名字了，比默认名更糟）；超长截断而不是拒绝。
   */
  rename(id: string, title: string): boolean {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry) return false;
    const trimmed = title.trim();
    if (!trimmed) return false;
    entry.tab.title = trimmed.slice(0, 40);

    this.deps.onChange();
    return true;
  }

  /** 关掉一个 tab。运行中且没确认 → `needsConfirm`，什么都不杀。 */
  async close(id: string, options: { force?: boolean } = {}): Promise<CloseResult> {
    const at = this.entries.findIndex((entry) => entry.tab.id === id);
    if (at < 0) return { closed: true };
    const entry = this.entries[at];
    const killable = entry.tab.status === "running" || entry.tab.status === "starting";
    if (killable && !options.force) return { closed: false, needsConfirm: true };
    this.entries.splice(at, 1);
    entry.session.dispose();
    this.deps.onChange();
    return { closed: true };
  }

  /** 停止进程但**留着这块屏**：输出还在，用户可以继续往上滚。 */
  async stop(id: string): Promise<void> {
    const entry = this.entries.find((item) => item.tab.id === id);
    if (!entry) return;
    entry.stopRequested = true;
    await entry.session.stop();
    // 状态由会话回报驱动：stop() 之后 running=false 且没有退出码 ⇒ "stopped"（我们关的）
    this.syncFromSession(entry);
    this.deps.onChange();
  }

  /** 新开一个 tab：旧的**一个都不动**。 */
  async create(input: CreateTabInput): Promise<CreateTabResult> {
    if (this.disposed) return { ok: false, reason: "store-disposed" };
    if (this.entries.length >= this.maxSessions) {
      return { ok: false, reason: "limit", max: this.maxSessions };
    }
    const program = input.program.trim();
    if (!program) return { ok: false, reason: "empty-program" };

    this.seq += 1;
    const bundle = (this.deps.makeBundle ?? createTerminal)({
      cols: FALLBACK_COLS,
      rows: FALLBACK_ROWS,
    });
    const entry: Entry = {
      tab: {
        id: `term-${this.seq}`,
        kind: input.kind ?? "local",
        // 默认**不给句子**：只给序号，名字由界面按语言渲染（"会话 1"）。
        title: null,
        index: this.seq,
        program,
        args: input.args,
        ptyId: null,
        status: "starting",
        exitCode: null,
        exitSignal: null,
        error: null,
        cols: bundle.term.cols,
        rows: bundle.term.rows,
        renderer: bundle.renderer,
      },
      session: new TerminalSession(bundle, this.deps.transport, {
        onState: () => this.onSessionState(entry),
        onError: (message) => this.onSessionError(entry, message),
      }),
      bundle,
      beginning: false,
      stopRequested: false,
      args: input.args,
      cwd: input.cwd ?? null,
    };
    entry.session.attach();
    // 先登记再启动：两个"新建"连着点也不会超到 9 个（超了就是同时起 9 条真进程）
    this.entries.push(entry);
    this.deps.onChange();
    // **不在这里起进程**：还没挂到 DOM 上，尺寸只能瞎猜，程序会按错宽度排版（C-05）。
    // 进程由 mount() 在拿到宿主元素之后起来。
    return { ok: true, id: entry.tab.id };
  }

  /** 页面卸载：收掉全部会话。**只在这条路径上调用**。 */
  disposeAll(): void {
    if (this.disposed) return;
    this.disposed = true;
    const entries = this.entries;
    this.entries = [];
    entries.forEach((entry) => entry.session.dispose());
    this.deps.onChange();
  }

  /** 真起进程。只由 `mount()` 在拿到 DOM 元素之后调用一次（状态一离开 starting 就不会再进来）。 */
  private async begin(entry: Entry): Promise<void> {
    if (entry.beginning || entry.tab.status !== "starting") return;
    entry.beginning = true;
    // 尺寸两头对齐之后再把进程起来：prompt_toolkit 这类程序按**当时**的列数排版，
    // 先起再改要再吃一次 SIGWINCH，中间的输出就按错宽度换行了
    entry.session.fit();
    await entry.session.start(entry.tab.program, entry.args, entry.cwd);
    // 起不来时 session 自己已经报过（屏里一行 + onError），这里只负责别把异常抛回调用方
    // 起进程这期间用户可能已经把 tab 关掉了：那时 entry 已不在表里，写回状态没有意义
    if (!this.entries.includes(entry)) return;
    this.syncFromSession(entry);
    this.deps.onChange();
  }

  /** 会话自己报的状态（running / 退出码）落进 tab；`stop()` 之后没有退出码，那是"我们关的"。 */
  private syncFromSession(entry: Entry): void {
    const state = entry.session.state;
    entry.tab.ptyId = entry.session.sessionId;
    if (state.cols) entry.tab.cols = state.cols;
    if (state.rows) entry.tab.rows = state.rows;
    if (state.running) {
      entry.tab.status = "running";
      entry.tab.exitCode = null;
      return;
    }
    /**
     * **还没起过进程**：这次通知只是尺寸/初始化，不是生命周期事件。
     * 让它去翻状态就会出现"量到了尺寸 ⇒ 反而永远不起进程"（真浏览器必现、jsdom 必不现）。
     */
    if (!entry.session.started) {
      if (state.cols) entry.tab.cols = state.cols;
      if (state.rows) entry.tab.rows = state.rows;
      return;
    }
    if (entry.tab.status !== "running" && entry.tab.status !== "starting") return;
    entry.tab.exitCode = state.exitCode;
    entry.tab.exitSignal = entry.session.exitSignal;
    if (state.exitCode !== null) {
      entry.tab.status = "exited";
      return;
    }
    /**
     * 没有退出码的两种情况以前被混成一句"会话已停止"（橙色灯），把一次异常死亡说成
     * 是用户自己关的——第八十六轮排查空白屏时就在这里被带偏过一次。
     * 现在只有**我们自己调过 stop()** 才叫"已停止"，其余一律算"已退出"并尽量带上信号名。
     */
    entry.tab.status = entry.stopRequested ? "stopped" : "exited";
  }

  private onSessionState(entry: Entry): void {
    this.syncFromSession(entry);
    this.deps.onChange();
  }

  private onSessionError(entry: Entry, message: string): void {
    entry.tab.error = message;
    entry.tab.status = "failed";
    this.deps.onChange();
  }
}
