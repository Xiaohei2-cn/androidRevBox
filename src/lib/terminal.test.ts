/**
 * 用**真 xterm.js 的缓冲区**验显示语义（FT0.3）。
 *
 * 为什么要专门写这份：用户的顾虑是「自研结构化会有一堆显示性 bug」。换开源内核之后这些行为
 * 不再由我们实现，但**接对了没有仍然由我们负责**——喂错顺序、忘开字素模式、把备用屏当普通滚动，
 * 都是界面层的 bug，而且只有在这里才测得出来。
 *
 * 分工说清楚（别把这份测试当成「渲染测过了」）：
 * - jsdom 没有布局，所以断言的是**缓冲区模型**：字符、列宽、光标位置、属性、备用屏切换；
 * - 像素与字体度量那一半归 `docs/frida_terminal_phases.md` §5 的手工矩阵（R-01）。
 */
import { Terminal } from "@xterm/xterm";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { beforeEach, describe, expect, it } from "vitest";

import { createTerminal, measureFit, type TerminalBundle } from "./terminal";

const E = "\u001b";

/** 写完必须等 xterm 的回调：它内部分帧处理，同步断言会读到半截缓冲 */
function feed(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, () => resolve()));
}

function lineOf(buffer: Terminal["buffer"]["active"], row = 0): string {
  return buffer.getLine(row)?.translateToString(true) ?? "";
}

/**
 * 挂到 DOM 上再测。搜索要设选区、粘贴要写输入辅助元素，这些 API 在未 open 的终端上
 * 直接踩空（第一版就是没挂载，于是 search/paste 两条腿报的是"xterm 坏了"，
 * 其实是我们没给它 DOM）——这本身就是一条值得留下的坑。
 */
function mounted(cols = 40, rows = 8): TerminalBundle {
  const bundle = createTerminal({ cols, rows });
  const host = document.createElement("div");
  // jsdom 没有布局，给个尺寸免得 xterm 量到 0 就放弃
  Object.defineProperty(host, "clientWidth", { value: cols * 8, configurable: true });
  Object.defineProperty(host, "clientHeight", { value: rows * 16, configurable: true });
  document.body.appendChild(host);
  bundle.term.open(host);
  return bundle;
}

describe("xterm 缓冲区里的终端语义", () => {
  let bundle: TerminalBundle;
  let term: Terminal;
  beforeEach(() => {
    bundle = mounted();
    term = bundle.term;
  });

  it("C-02 \\r 覆盖同一行：屏幕上那一行最后留下的是什么就是什么", async () => {
    await feed(term, "a\rbb\rccc\n");
    expect(lineOf(term.buffer.active).trimEnd()).toBe("ccc");
  });

  it("C-03 光标后退（CUB）与绝对定位（CUP）真的动了光标，不是把字符接在末尾", async () => {
    await feed(term, "abc");
    expect(term.buffer.active.cursorX).toBe(3);
    await feed(term, `${E}[2DX`); // 退两格再写：中间的 b 被 X 替掉
    expect(lineOf(term.buffer.active)).toBe("aXc");
    await feed(term, `${E}[3;1HZ`); // 第 3 行第 1 列
    expect(lineOf(term.buffer.active, 2)).toBe("Z");
  });

  it("C-04 备用屏：进去写新的，退出来旧内容还在（top/less 退出就靠这个）", async () => {
    await feed(term, "normal line\n");
    await feed(term, `${E}[?1049hALT SCREEN\n`);
    expect(term.buffer.active.type).toBe("alternate");
    const dump = Array.from({ length: term.rows }, (_, i) => lineOf(term.buffer.active, i)).join("|");
    expect(dump, `备用屏里没看到内容：${dump}`).toContain("ALT SCREEN");
    expect(lineOf(term.buffer.normal)).toBe("normal line");
    await feed(term, `${E}[?1049l`);
    expect(term.buffer.active.type).toBe("normal");
    expect(lineOf(term.buffer.active)).toBe("normal line");
  });

  it("C-01 256 色与真彩色进了缓冲区的**颜色值**，不只是文本里还留着转义", async () => {
    await feed(term, `${E}[38;5;208mORANGE${E}[0m ${E}[38;2;10;20;30mTRUE${E}[0m\n`);
    const line = term.buffer.active.getLine(0);
    // 调色板色：缓冲区里存的就是 208 这个索引
    expect(line?.getCell(0)?.getChars()).toBe("O");
    expect(line?.getCell(0)?.getFgColor()).toBe(208);
    expect(line?.getCell(0)?.getFgColorMode()).toBeGreaterThan(0);
    // 真彩色：0xRRGGBB 原值（10,20,30 → 0x0a141e）。ORANGE(6) + 复位 + 一个空格 ⇒ 第 7 列
    expect(line?.getCell(7)?.getChars()).toBe("T");
    expect(line?.getCell(7)?.getFgColor()).toBe(0x0a141e);
    // SGR 0 复位之后写的字必须回到默认前景，否则会一路染色下去。
    // 列位按内容现算，不写死：`\n` 单独只下移不回首（那是终端的真实语义，
    // 上一版把 GREEN 当成第二行第 0 列，测的就不是 xterm 而是我的假设）。
    await feed(term, `${E}[32mGREEN${E}[0m PLAIN\n`);
    const row1 = term.buffer.active.getLine(1);
    const text1 = row1?.translateToString(true) ?? "";
    const greenAt = text1.indexOf("GREEN");
    const plainAt = text1.indexOf("PLAIN");
    expect(greenAt).toBeGreaterThanOrEqual(0);
    expect(plainAt).toBeGreaterThan(greenAt);
    const green = row1?.getCell(greenAt);
    const plainAfterReset = row1?.getCell(plainAt);
    expect(green?.getChars()).toBe("G");
    expect(plainAfterReset?.getChars()).toBe("P");
    // 复位生效的判据：同一行里，绿色的格和复位后的格颜色值不同
    expect(green?.getFgColor()).not.toBe(plainAfterReset?.getFgColor());

    // 序列化能把它带回原样（切 tab / 重开界面恢复用的就是这条）
    const out = await bundle.serialize.serialize();
    expect(out).toContain("ORANGE");
    expect(out).toContain("PLAIN");
  });

  it("C-08 中文一个字占两列，中英混排不半格错位（字素模式已开）", async () => {
    await feed(term, "中a\n");
    const cell0 = term.buffer.active.getLine(0)?.getCell(0);
    expect(cell0?.getChars()).toBe("中");
    expect(cell0?.getWidth()).toBe(2);
    expect(lineOf(term.buffer.active).trimEnd()).toBe("中a");
  });

  it("C-07 程序开了 bracketed paste 时，多行粘贴不被当成连按回车", async () => {
    const chunks: string[] = [];
    term.onData((d) => chunks.push(d));
    await feed(term, `${E}[?2004h`);
    term.paste("a\nb");
    const sent = chunks.join("");
    expect(sent).toContain(`${E}[200~`);
    expect(sent).toContain(`${E}[201~`);
  });

  it("C-11 缓冲区搜索找得到，也算得出找不到", async () => {
    await feed(term, "one\ntwo sign=deadbeef\nthree\n");
    expect(bundle.search.findNext("deadbeef")).toBe(true);
    expect(bundle.search.findNext("nope-not-here")).toBe(false);
  });

  it("C-12 序列化出来的文本能重放到另一台终端（切 tab / 重开界面不留白）", async () => {
    await feed(term, `${E}[1;33mBOLD${E}[0m tail\nsecond\n`);
    const snapshot = await bundle.serialize.serialize();
    const other = new Terminal({ cols: 40, rows: 8, allowProposedApi: true });
    other.loadAddon(new UnicodeGraphemesAddon());
    await feed(other, snapshot);
    expect(lineOf(other.buffer.active).trim()).toContain("BOLD tail");
    // 快照是"整屏 + 光标定位"，重放后行内容对得上就算恢复成功
    expect(Array.from({ length: other.rows }, (_, i) => lineOf(other.buffer.active, i)).join("\n"))
      .toContain("second");
  });

  it("R-02 的退路是真的：拿不到 WebGL 就安静退回 DOM 渲染器，而不是把会话炸掉", () => {
    // jsdom 里没有 WebGL 上下文——这一条正好演练退路。真机 WKWebView 上到底走哪条，
    // 归 §5 手工矩阵，不许在这儿宣布"实测通过"。
    expect(bundle.renderer).toBe("dom");
  });

  it("R-03：容器量不到尺寸时不谎报尺寸（隐藏 tab 的真实情况）", () => {
    const bare = createTerminal({ cols: 10, rows: 4 });
    expect(measureFit(bare)).toBeNull();
  });

  it("界面不许把 0 列 0 行传下去：程序问到的尺寸必须是个能用的值", () => {
    const tiny = createTerminal({ cols: 0, rows: 0 });
    expect(tiny.term.cols).toBeGreaterThanOrEqual(20);
    expect(tiny.term.rows).toBeGreaterThanOrEqual(4);
  });
});
