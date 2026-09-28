/**
 * 终端正文导出（C-15 终端那一半）。用**真 xterm**：这里要证的正是"从缓冲区取出来的是
 * 屏幕上那些字"，桩掉终端就只剩桩的形状。
 */
import { beforeEach, describe, expect, it } from "vitest";

import { createTerminal } from "./terminal";
import { collectTranscript, transcriptFileName, transcriptText } from "./terminalTranscript";

const ENCODE = new TextEncoder();

function termWith(data: string, cols = 30, rows = 6) {
  const bundle = createTerminal({ cols, rows });
  const host = document.createElement("div");
  document.body.appendChild(host);
  bundle.term.open(host);
  bundle.term.write(ENCODE.encode(data));
  return bundle;
}

/** xterm 的写入是异步的（要等一次刷新），测试里用它的写回调等到位 */
async function flush(term: ReturnType<typeof createTerminal>["term"]) {
  await new Promise<void>((resolve) => term.write(ENCODE.encode(""), () => resolve()));
}

describe("终端缓冲区导出", () => {
  let bundle: ReturnType<typeof createTerminal>;
  beforeEach(() => {
    bundle = undefined as unknown as ReturnType<typeof createTerminal>;
  });

  it("带颜色与中文的正文取出来是纯文本：一个转义字符都不留", async () => {
    bundle = termWith("sign=\u001b[38;5;208mABC123\u001b[0m 中文值\r\n下一行\r\n");
    await flush(bundle.term);
    const snapshot = collectTranscript(bundle.term);
    expect(snapshot.lines[0]).toContain("sign=ABC123 中文值");
    expect(snapshot.lines.join("\n").includes("\u001b")).toBe(false);
    bundle.dispose();
  });

  it("顺序从旧到新，尾部空行不留", async () => {
    bundle = termWith("L1\r\nL2\r\nL3\r\n");
    await flush(bundle.term);
    const snapshot = collectTranscript(bundle.term);
    expect(snapshot.lines.slice(0, 3)).toEqual(["L1", "L2", "L3"]);
    expect(snapshot.lines[snapshot.lines.length - 1]).toBe("L3");
    bundle.dispose();
  });

  /** 屏幕上被覆盖过的那一行，导出的是**最终形态**（这就是"存留在屏幕上的那些行"）。 */
  it("被 \\r 覆盖过的行导出覆盖后的结果", async () => {
    bundle = termWith("progress-1\rprogress-2\r\n", 20, 4);
    await flush(bundle.term);
    const snapshot = collectTranscript(bundle.term);
    expect(snapshot.lines[0]).toBe("progress-2");
    expect(snapshot.lines.join("\n")).not.toContain("progress-1");
    bundle.dispose();
  });

  it("搜索框里有字就只存匹配的行，并说清一共多少行", async () => {
    bundle = termWith("GET /a\r\nsign=deadbeef\r\nGET /b\r\n");
    await flush(bundle.term);
    const snapshot = collectTranscript(bundle.term, "SIGN=");
    expect(snapshot.lines).toEqual(["sign=deadbeef"]);
    expect(snapshot.totalLines).toBe(3);
    expect(snapshot.query).toBe("sign=");
    bundle.dispose();
  });

  it("没命中就是空：不写文件也不报错（空串，不是一个换行）", async () => {
    bundle = termWith("GET /a\r\n");
    await flush(bundle.term);
    const snapshot = collectTranscript(bundle.term, "nothing-here");
    expect(snapshot.lines).toEqual([]);
    expect(transcriptText(snapshot)).toBe("");
    bundle.dispose();
  });

  it("建议文件名：取程序名、清掉不能进文件名的字符", () => {
    const at = new Date(2026, 8, 28, 21, 45);
    expect(transcriptFileName("/bin/zsh", at)).toBe("terminal-zsh-20260928-2145.txt");
    expect(transcriptFileName("frida_tools.repl", at)).toBe("terminal-frida_tools.repl-20260928-2145.txt");
    // 取的是**最后一段**（basename），不是整条路径：文件名里出现斜杠会直接改变落盘位置
    expect(transcriptFileName("../weird name!/x y", at)).toBe("terminal-xy-20260928-2145.txt");
    expect(transcriptFileName("my script; rm -rf $HOME", at)).toBe("terminal-myscriptrm-rfHOME-20260928-2145.txt");
    expect(transcriptFileName("", at)).toBe("terminal-session-20260928-2145.txt");
  });
});
