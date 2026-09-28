/**
 * 终端正文导出（C-15 的终端那一半，D090 的口径）。
 *
 * 只做一件事：把 xterm **缓冲区里现在有的那些行**取成纯文本。
 * 为什么是纯文本而不是 `serialize()`：serialize 会把 SGR/光标序列一起写出来，
 * 那种文件贴进工单就是一屏乱码；用户要的是"屏幕上看到的那些字"。
 * 颜色/属性本来就在 xterm 的缓冲区里，不需要我们解析——这正是换开源内核之后省事的地方。
 *
 * 三条口径：
 * 1. **自动落盘仍然不做**（重构文档 §3.7 一字未松）：只有用户点过一次「保存日志」并确认路径才写。
 * 2. **搜索框里有字就只存匹配的行**——与结构化控制台的"存过滤后留在屏幕上的那些行"同一件事。
 * 3. 不设人为行数上限：缓冲区本身有 `scrollback` 上界（默认 2 万行），
 *    再往上有一道 Rust 侧 64 MiB 的写盘上限兜底；在这里再编一个行数限制只会多一个没人懂的报错。
 */
import type { Terminal } from "@xterm/xterm";

export interface TranscriptSnapshot {
  /** 要写出去的行（已按过滤条件筛过、去掉尾部空行） */
  lines: string[];
  /** 缓冲区里一共多少行（用于"筛掉了多少"的说明） */
  totalLines: number;
  /** 生效的过滤词（小写化后），空 = 没过滤 */
  query: string;
}

/**
 * 读缓冲区。`trimRight` 交给 xterm（它知道哪些是填充的空格），
 * 我们只做两件不涉语义的事：按过滤词筛行、去掉尾部连续空行。
 */
export function collectTranscript(term: Terminal, rawQuery = ""): TranscriptSnapshot {
  const buffer = term.buffer.active;
  const query = rawQuery.trim().toLowerCase();
  const all: string[] = [];
  for (let index = 0; index < buffer.length; index += 1) {
    const line = buffer.getLine(index)?.translateToString(true);
    all.push(line ?? "");
  }
  while (all.length > 0 && all[all.length - 1] === "") all.pop();

  const matched = query ? all.filter((line) => line.toLowerCase().includes(query)) : all;
  return { lines: matched, totalLines: all.length, query };
}

/** 写出去的内容：行 + 末尾一个换行（空缓冲区就是空串，不是 "\n"） */
export function transcriptText(snapshot: TranscriptSnapshot): string {
  if (snapshot.lines.length === 0) return "";
  return `${snapshot.lines.join("\n")}\n`;
}

/** 文件名的建议值：程序名 + 时间，不带扩展名以外的空格 */
export function transcriptFileName(program: string, at = new Date()): string {
  const base = program
    .split("/")
    .filter(Boolean)
    .pop()
    ?.replace(/[^A-Za-z0-9._-]/g, "")
    ?.slice(0, 40);
  const stamp = `${at.getFullYear()}${String(at.getMonth() + 1).padStart(2, "0")}${String(at.getDate()).padStart(2, "0")}-${String(at.getHours()).padStart(2, "0")}${String(at.getMinutes()).padStart(2, "0")}`;
  return `terminal-${base || "session"}-${stamp}.txt`;
}
