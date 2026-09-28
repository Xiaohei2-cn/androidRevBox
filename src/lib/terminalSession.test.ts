/**
 * 会话控制层（xterm ↔ PTY）的单测：这一层是"我们自己写的那一段"，
 * 所以它有资格也有必要被自动化测——`terminal.test.ts` 证的是 xterm 的行为，
 * 这里证的是**我们有没有把它接对**。
 */
import { beforeEach, describe, expect, it } from "vitest";

import { createTerminal } from "./terminal";
import { TerminalSession } from "./terminalSession";
import type { PtyExit, PtyTransport } from "@/api/pty";

interface Call {
  kind: "open" | "write" | "resize" | "close";
  detail: string;
}

function fakeTransport() {
  const calls: Call[] = [];
  let onData: ((bytes: Uint8Array) => void) | null = null;
  let exitHandler: ((exit: PtyExit) => void) | null = null;
  const transport: PtyTransport = {
    async open(args) {
      onData = args.onData;
      calls.push({ kind: "open", detail: `${args.program} ${args.cols}x${args.rows}` });
      return { id: "pty-1", pid: 4242, cols: args.cols, rows: args.rows, term: "xterm-256color" };
    },
    async write(id, bytes) {
      calls.push({ kind: "write", detail: `${id}:${Array.from(bytes).join(",")}` });
    },
    async resize(id, cols, rows) {
      calls.push({ kind: "resize", detail: `${id} ${cols}x${rows}` });
    },
    async close(id) {
      calls.push({ kind: "close", detail: id });
    },
    async defaultProgram() {
      return "/bin/sh";
    },
    async onExit(handler) {
      exitHandler = handler;
      return () => {
        exitHandler = null;
      };
    },
  };
  return { transport, calls, emitData: (b: Uint8Array) => onData?.(b), fireExit: (e: PtyExit) => exitHandler?.(e) };
}

/**
 * 挂载一个终端。返回 host 是因为 FitAddon 量的就是 `term.element`（也就是这个容器）：
 * 想模拟"界面变宽"必须改它的尺寸，改它的父节点是量不到的
 * （第一版就是改错了节点，于是"resize 没生效"看起来像代码 bug，其实是测试没测到东西）。
 */
function mountSession(cols = 20, rows = 5, width = 0, height = 0) {
  const bundle = createTerminal({ cols, rows });
  const host = document.createElement("div");
  Object.defineProperty(host, "clientWidth", { value: width, configurable: true });
  Object.defineProperty(host, "clientHeight", { value: height, configurable: true });
  document.body.appendChild(host);
  bundle.term.open(host);
  return { bundle, host };
}

describe("终端会话控制层", () => {
  let fake: ReturnType<typeof fakeTransport>;
  beforeEach(() => {
    fake = fakeTransport();
  });

  it("起会话时传的是能量到的尺寸，绝不是 0 列 0 行", async () => {
    const { bundle } = mountSession(20, 5, 0, 0);
    const session = new TerminalSession(bundle, fake.transport);
    await session.start("/bin/sh", [], null);
    expect(fake.calls[0].kind).toBe("open");
    expect(fake.calls[0].detail).toMatch(/ [1-9]\d*x[1-9]\d*$/);
    session.dispose();
  });

  /**
   * 这条就是"中文变问号"那类 bug 的防线：设备回来的字节可能把一帧 UTF-8 从中间切开，
   * 中间只要做过一次文本解码，字符就再也拼不回来了。
   */
  it("分两块送来的一帧 UTF-8 拼回同一个字（不做任何中途解码）", async () => {
    const { bundle } = mountSession();
    const session = new TerminalSession(bundle, fake.transport);
    await session.start("/bin/sh", [], null);
    const 中 = new TextEncoder().encode("中");
    // 9 节的 UTF-8 序列？中文是 3 字节：拆成 1 + 2 两块送
    session.feed(中.slice(0, 1));
    session.feed(中.slice(1));
    await new Promise((r) => setTimeout(r, 30));
    const line = bundle.term.buffer.active.getLine(0)?.translateToString(true) ?? "";
    expect(line.trim()).toBe("中");
    session.dispose();
  });

  /** 键盘：xterm 给了什么就送什么，一个字节都不补（Ctrl-C 是 0x03，不是"我们代为发个中断"）。 */
  it("键盘文本按 UTF-8 原样写进去，不补换行也不翻译控制键", async () => {
    const { bundle } = mountSession();
    const session = new TerminalSession(bundle, fake.transport);
    session.attach();
    await session.start("/bin/sh", [], null);
    bundle.term.input("\u0003"); // Ctrl-C
    await new Promise((r) => setTimeout(r, 10));
    const writes = fake.calls.filter((c) => c.kind === "write");
    expect(writes.length).toBe(1);
    expect(writes[0].detail.endsWith(":3")).toBe(true);
    session.dispose();
  });

  it("量不到尺寸时不许把 0 传给 tty（R-03）", async () => {
    const { bundle } = mountSession();
    const session = new TerminalSession(bundle, fake.transport);
    await session.start("/bin/sh", [], null);
    // 量不到（隐藏 tab、容器宽 0）：不改尺寸也不往下传，返回 false 让调用方别误报
    expect(session.fit(() => null)).toBe(false);
    expect(fake.calls.some((c) => c.kind === "resize")).toBe(false);
    session.dispose();
  });

  it("尺寸变了要两头一起改：缓冲区 + tty 的 winsize", async () => {
    const { bundle } = mountSession(20, 5, 20 * 8, 5 * 16);
    const session = new TerminalSession(bundle, fake.transport);
    await session.start("/bin/sh", [], null);
    // 注入量到的新尺寸（= 用户把窗口拖宽）：jsdom 里没有布局，真量只能得 0，
    // 所以这里验的是"接到新尺寸之后两头都跟着改"这段判断，不是"能不能量到"
    expect(session.fit(() => ({ cols: 80, rows: 50 }))).toBe(true);
    await new Promise((r) => setTimeout(r, 10));
    expect(bundle.term.cols).toBeGreaterThan(20);
    expect(fake.calls.some((c) => c.kind === "resize" && c.detail.includes(`${bundle.term.cols}x`))).toBe(true);
    session.dispose();
  });

  it("退出事件只认自己那条会话（多 tab 并存时不许互相翻状态）", async () => {
    const { bundle } = mountSession();
    const states: boolean[] = [];
    const session = new TerminalSession(bundle, fake.transport, {
      onState: (s) => states.push(s.running),
    });
    await session.start("/bin/sh", [], null);
    fake.fireExit({ id: "pty-999", exitCode: 1, signal: null });
    expect(states.at(-1)).toBe(true); // 别人的退出不能翻我们的状态
    fake.fireExit({ id: "pty-1", exitCode: 0, signal: null });
    expect(states.at(-1)).toBe(false);
    session.dispose();
  });

  it("关会话必须真的关到传输层一次（R-08：界面关了不许留活进程）", async () => {
    const { bundle } = mountSession();
    const session = new TerminalSession(bundle, fake.transport);
    await session.start("/bin/sh", [], null);
    await session.stop();
    expect(fake.calls.filter((c) => c.kind === "close")).toHaveLength(1);
    session.dispose();
    expect(fake.calls.filter((c) => c.kind === "close")).toHaveLength(1); // 不许关第二次
  });
});
