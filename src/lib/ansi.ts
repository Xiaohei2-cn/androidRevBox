/**
 * 控制台里的 ANSI 转义处理（第五十六轮：frida 控制台把脚本的颜色码当文字显示）。
 *
 * 现场：脚本打印 hexdump 时发 `\x1b[0;32m00000000\x1b[0m` 这类 SGR 序列，而 frida 的
 * `console.log` 走 NDJSON 的 `msg` 字段原样回到宿主 —— 宿主直接当文本渲染，用户看到的就是
 * `[0;32m00000000[0m` 这种垃圾，hexdump 的列对齐也被这些字符撑乱。
 *
 * 现状（第九十六轮）：结构化控制台与它的 AnsiText 渲染器已整体退役，
 * 这里只剩 **stripAnsi** —— 二进制托管页把 `--help` 文本洗成可读段落用。
 * 「SGR 翻样式」那一半（ansiRuns）随 AnsiText 一起删了：读彩色输出去真终端页。
 */

/* eslint-disable no-control-regex -- 这里要处理的**就是**控制字符：ESC/BEL 出现在
   终端输出里是常态，正则里的 \x1b 不是写错，是这门协议的定界符。 */
/** CSI / OSC / 单字符转义：都要吃掉，不然界面上就是乱码 */
const ANSI_ALL =
  /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;

/** 剥掉所有转义，得到"用户实际看到的文字" */
export function stripAnsi(s: string): string {
  return s.replace(ANSI_ALL, "");
}
