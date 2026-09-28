/**
 * 终端会话控制层（FT0.3）：把 xterm 的键盘/尺寸事件接到 PTY 传输上。
 *
 * 为什么单独一层（而不是全写在组件里）：这三条腿都必须能**脱离 WebView 被测**——
 * 字节喂进去缓冲区对不对（`terminal.test.ts` 已经证明会出错的是我们接的那一段，
 * 不是 xterm），尺寸变化会不会被静默吞掉，隐藏 tab 时量到 0 怎么处理。
 * 组件只负责挂载与卸载。
 */
import { measureFit, type TerminalBundle } from "./terminal";
import type { PtyTransport } from "@/api/pty";

export interface TerminalSessionState {
  running: boolean;
  exitCode: number | null;
  cols: number;
  rows: number;
}

export interface TerminalSessionEvents {
  onState?: (state: TerminalSessionState) => void;
  onError?: (message: string) => void;
}

const ENCODER = new TextEncoder();

export class TerminalSession {
  private id: string | null = null;

  private lastCols = 0;

  private lastRows = 0;

  private disposed = false;

  private detachData: (() => void) | null = null;

  private detachExit: (() => void) | null = null;

  private state: TerminalSessionState = { running: false, exitCode: null, cols: 0, rows: 0 };

  constructor(
    private readonly bundle: TerminalBundle,
    private readonly transport: PtyTransport,
    private readonly events: TerminalSessionEvents = {},
  ) {}

  get sessionId(): string | null {
    return this.id;
  }

  /** 键盘、粘贴、IME 全都从 onData 出来：那是 xterm 已经按模式编码好的字节，我们不补任何控制码。 */
  attach(): void {
    this.detachData = this.bundle.term.onData((data) => {
      void this.writeText(data);
    }).dispose;
    // 程序自己请求改标题（OSC 0/2）时不许崩：xterm 会走这里，我们暂时只让它写进实例
    this.bundle.term.onTitleChange(() => undefined);
  }

  async start(program: string, args: string[], cwd: string | null): Promise<void> {
    if (this.id) return;
    // 起会话前先按界面量一次尺寸：量不到（隐藏）就用当前 cols/rows，绝不传 0（R-03）
    const proposed = measureFit(this.bundle) ?? {
      cols: this.bundle.term.cols,
      rows: this.bundle.term.rows,
    };
    this.bundle.term.resize(proposed.cols, proposed.rows);
    try {
      const opened = await this.transport.open({
        program,
        args,
        cwd,
        cols: proposed.cols,
        rows: proposed.rows,
        onData: (bytes) => this.feed(bytes),
      });
      this.id = opened.id;
      this.lastCols = opened.cols;
      this.lastRows = opened.rows;
      this.detachExit = (await this.transport.onExit((exit) => {
        if (exit.id !== this.id) return;
        this.update({ running: false, exitCode: exit.exitCode ?? null });
      })) ?? null;
      this.update({ running: true, exitCode: null, cols: opened.cols, rows: opened.rows });
    } catch (error) {
      this.events.onError?.(String((error as Error)?.message ?? error));
    }
  }

  /** 设备/子进程回来的原始字节直接喂 xterm：**不解码、不按行切、不补换行**。 */
  feed(bytes: Uint8Array): void {
    this.bundle.term.write(bytes);
  }

  /**
   * 界面尺寸变了就同时改两头：xterm 的缓冲区 + tty 的 winsize。
   * 返回 false = 这次量不到（tab 隐藏、容器宽 0），**保持原尺寸不动**——
   * 把 0 传下去会让程序以为自己 0 列（`stty`/curses 直接算错）。
   */
  /**
   * `measure` 可注入：生产的尺寸要靠真实布局量（jsdom 没有布局，`getBoundingClientRect`
   * 全给 0，于是这一条分支在 jsdom 里只能测"量不到"那一半，不能假装测过整条）。
   */
  fit(measure: () => { cols: number; rows: number } | null = () => measureFit(this.bundle)): boolean {
    const proposed = measure();
    if (!proposed) return false;
    if (proposed.cols === this.lastCols && proposed.rows === this.lastRows) return true;
    this.bundle.term.resize(proposed.cols, proposed.rows);
    this.lastCols = proposed.cols;
    this.lastRows = proposed.rows;
    this.update({ cols: proposed.cols, rows: proposed.rows });
    if (this.id) {
      void this.transport.resize(this.id, proposed.cols, proposed.rows).catch((error) => {
        this.events.onError?.(String((error as Error)?.message ?? error));
      });
    }
    return true;
  }

  async stop(): Promise<void> {
    const id = this.id;
    if (!id) return;
    this.id = null;
    try {
      await this.transport.close(id);
    } catch (error) {
      this.events.onError?.(String((error as Error)?.message ?? error));
    }
    this.update({ running: false, exitCode: null });
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.detachData?.();
    this.detachExit?.();
    const id = this.id;
    this.id = null;
    if (id) void this.transport.close(id).catch(() => undefined);
    this.bundle.dispose();
  }

  private async writeText(data: string): Promise<void> {
    if (!this.id) return;
    try {
      await this.transport.write(this.id, ENCODER.encode(data));
    } catch (error) {
      this.events.onError?.(String((error as Error)?.message ?? error));
    }
  }

  private update(patch: Partial<TerminalSessionState>): void {
    this.state = { ...this.state, ...patch };
    this.events.onState?.({ ...this.state });
  }
}
