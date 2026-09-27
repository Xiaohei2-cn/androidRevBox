/**
 * 一行 shell 输入 → executable + args（纯函数，不执行任何 shell 展开）。
 *
 * 从 `@/api/task` 搬出来只为了一个原因：那个模块连着 Tauri 的 invoke/event 运行时，
 * 而这个分词是纯逻辑，别处（二进制托管的参数栏）也要用同一套引号规则。
 * 留在原地就等于逼每个复用者都拖进一份 IPC 依赖，或者——更糟——自己另抄一份。
 */
export function tokenizeCommand(line: string): { executable: string; args: string[] } | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  const tokens: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let has = false;
  for (const ch of trimmed) {
    if (quote) {
      if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      has = true;
    } else if (/\s/.test(ch)) {
      if (has || current) {
        tokens.push(current);
        current = "";
        has = false;
      }
    } else {
      current += ch;
    }
  }
  if (has || current) tokens.push(current);
  if (tokens.length === 0) return null;
  const [executable, ...args] = tokens;
  return { executable, args };
}
