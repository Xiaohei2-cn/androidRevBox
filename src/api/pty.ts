/**
 * 真终端的前端出口（FT0.2/0.3）。
 *
 * 字节走 `Channel` 而不是全局事件的两条理由写在 Rust 那边（`commands/pty.rs`）：
 * 一个会话一条通道、载荷是 Raw 不用 base64。这里对应的是**必须按 ArrayBuffer 收**——
 * 收到 `Uint8Array` 就直接喂 xterm，中途不做任何文本解码：半截的 UTF-8 序列
 * 一旦在这里被 `TextDecoder` 消化掉，界面上就会出现"中文变成问号"那种查不动的 bug。
 */
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { invokeCommand } from "./client";

export interface PtyOpened {
  id: string;
  pid: number | null;
  cols: number;
  rows: number;
  term: string;
}

export interface PtyExit {
  id: string;
  exitCode: number | null;
  signal: string | null;
}

/** 会话的字节契约。抽成接口是为了让控制层能脱离 tauri 被单测（假 api 喂假字节）。 */
export interface PtyTransport {
  open(args: {
    program: string;
    args: string[];
    cwd: string | null;
    cols: number;
    rows: number;
    onData: (bytes: Uint8Array) => void;
  }): Promise<PtyOpened>;
  write(id: string, bytes: Uint8Array): Promise<void>;
  resize(id: string, cols: number, rows: number): Promise<void>;
  close(id: string): Promise<void>;
  /** 默认起什么程序（本机 $SHELL / COMSPEC）。放在接口里是为了让控制层的测试也能注入它。 */
  defaultProgram(): Promise<string>;
  onExit(handler: (exit: PtyExit) => void): Promise<() => void>;
}

export const ptyApi: PtyTransport = {
  async open({ program, args, cwd, cols, rows, onData }) {
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (payload) => onData(new Uint8Array(payload));
    return invokeCommand<PtyOpened>("pty_open", {
      args: { program, args, cwd, cols, rows },
      onData: channel,
    });
  },
  write(id, bytes) {
    //  keystroke 的量级用不着 Raw：Vec<u8> 走 JSON 就行（一次几个字节）
    return invokeCommand<void>("pty_write", { id, bytes: Array.from(bytes) });
  },
  resize(id, cols, rows) {
    return invokeCommand<void>("pty_resize", { id, cols, rows });
  },
  close(id) {
    return invokeCommand<void>("pty_close", { id });
  },
  /** 默认起什么程序：判据是本机的 $SHELL / COMSPEC，由桌面端回答，界面不猜。 */
  defaultProgram(): Promise<string> {
    return invokeCommand<string>("pty_default_program");
  },
  onExit(handler) {
    return listen<PtyExit>("pty://exit", (event) => handler(event.payload)).then((un) => () => void un());
  },
};
