/**
 * 控制台里的 ANSI 转义处理（第五十六轮：frida 控制台把脚本的颜色码当文字显示）。
 *
 * 现场：脚本打印 hexdump 时发 `\x1b[0;32m00000000\x1b[0m` 这类 SGR 序列，而 frida 的
 * `console.log` 走 NDJSON 的 `msg` 字段原样回到宿主 —— 宿主直接当文本渲染，用户看到的就是
 * `[0;32m00000000[0m` 这种垃圾，hexdump 的列对齐也被这些字符撑乱。
 *
 * 策略：
 * - **SGR（改色/加粗）翻成样式**，让本来精心配色的 hexdump 真的有色可看；
 * - **其它转义（光标移动、清行、OSC 链接…）一律丢掉**，绝不显示出来；
 * - 复制与关键字过滤都走"去色后的纯文本"，否则搜 `00000000` 会因为中间夹着 ESC 而搜不到。
 */

/* eslint-disable no-control-regex -- 这里要处理的**就是**控制字符：ESC/BEL 出现在
   终端输出里是常态，正则里的 \x1b 不是写错，是这门协议的定界符。 */
/** SGR 之外的 CSI / OSC / 单字符转义：都要吃掉，不然界面上就是乱码 */
const ANSI_ALL =
  /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;

/** 只挑出 SGR（`\x1b[ ... m`），参数是数字与分号 */
// （ESC 是 SGR 的起始字节；上面的文件级 disable 已覆盖这条正则）
const SGR = /\x1b\[([0-9;]*)m/g;

/** 剥掉所有转义，得到"用户实际看到的文字" */
export function stripAnsi(s: string): string {
  return s.replace(ANSI_ALL, "");
}

/** 一段连续同样式的文字 */
export interface AnsiRun {
  text: string;
  /** tailwind 文字色类名；null = 跟随所在轨道的默认色 */
  color: string | null;
  bold: boolean;
}

/** 常规 30–37 与亮色 90–97 都映射到同一族颜色：控制台底色是黑的，两套都要够亮 */
const FG: Record<number, string> = {
  30: "text-zinc-500",
  31: "text-rose-400",
  32: "text-emerald-400",
  33: "text-amber-300",
  34: "text-sky-400",
  35: "text-fuchsia-400",
  36: "text-cyan-300",
  37: "text-zinc-200",
  90: "text-zinc-500",
  91: "text-rose-400",
  92: "text-emerald-400",
  93: "text-amber-300",
  94: "text-sky-400",
  95: "text-fuchsia-400",
  96: "text-cyan-300",
  97: "text-zinc-100",
};

/**
 * 把字符串切成"带样式的片段"。
 *
 * 不变式：把所有片段的 text 拼起来 === `stripAnsi(原文)`。
 * 不认识的 SGR 参数（下划线、反白、256 色等）只丢样式，**不丢文字**。
 */
export function ansiRuns(s: string): AnsiRun[] {
  if (!s.includes("\u001b")) return [{ text: s, color: null, bold: false }];
  const runs: AnsiRun[] = [];
  let color: string | null = null;
  let bold = false;
  let last = 0;
  const push = (text: string) => {
    if (!text) return;
    const prev = runs[runs.length - 1];
    // 同一样式的相邻片段合并：hexdump 一行会有几十段，别把 DOM 撑爆
    if (prev && prev.color === color && prev.bold === bold) prev.text += text;
    else runs.push({ text, color, bold });
  };
  for (const m of s.matchAll(SGR)) {
    push(stripAnsi(s.slice(last, m.index)));
    last = (m.index ?? 0) + m[0].length;
    const codes = (m[1] ?? "").split(";").map((c) => (c === "" ? 0 : Number(c)));
    for (const code of codes) {
      if (code === 0) {
        color = null;
        bold = false;
      } else if (code === 1 || code === 22) {
        bold = code === 1;
      } else if (code === 39) {
        color = null;
      } else if (FG[code]) {
        color = FG[code];
      }
      // 其它（4 下划线、7 反白、38/48 扩展色…）：只忽略样式，文字照留
    }
  }
  push(stripAnsi(s.slice(last)));
  return runs;
}
