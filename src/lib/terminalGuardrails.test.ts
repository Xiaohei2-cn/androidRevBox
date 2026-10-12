/**
 * FT 的两条机器护栏（`docs/frida_terminal_phases.md` §6.2）。
 *
 * 为什么要这份文件：整个 FT 的理由就是一句话——**别再自己解析转义序列**。
 * 这句话躺在文档里只能靠记性，而本项目的口径是"记性不可复现，脚本可以"。
 * 于是钉两条：
 * 1. 生产代码里**不许新增**自己解析 ANSI/VT 的地方。退役队列中的文件逐个列名 + 写理由，
 *    并且**白名单里失效的条目也会让测试变红**——FT4 退役之后这张表必须跟着变小，
 *    否则"退役"会退化成"把注释改成已废弃、文件继续躺在白名单里"。
 * 2. 终端相关依赖只用 xterm.js 家族 + `portable-pty`；想换内核必须先把"xterm.js 哪里不够"写下来。
 *
 * 扫描前会把注释剥掉：FT0 之后每个终端文件都要写一段"为什么我们不解析"，
 * 那些话里全是转义字面量，不剥就会把解释当成实现。
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = process.cwd();
const GUARD_FILE = "src/lib/terminalGuardrails.test.ts";

/** 递归列出 src 下的 .ts/.tsx（跳过测试与本文件） */
function productionSources(): string[] {
  const found: string[] = [];
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === "node_modules" || entry.name === "dist") continue;
        walk(full);
        continue;
      }
      if (!/\.(ts|tsx)$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) continue;
      found.push(full);
    }
  };
  walk(path.join(ROOT, "src"));
  return found.sort();
}

function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^[ \t]*\/\/.*$/gm, "")
    // 行尾注释：只在引号之前切，免得把字符串里的 // 当成注释吃掉
    .replace(/([^"'`])\/\/[^"'`]*$/gm, "$1");
}

function relative(full: string): string {
  return path.relative(ROOT, full).split(path.sep).join("/");
}

/** 自己解析/渲染转义序列的三种形状 */
function parsingSignatures(text: string): string[] {
  const hits: string[] = [];
  if (text.match(/\\(?:x1b|u001b|001b|033)/)) hits.push("代码里出现转义序列字面量");
  if (text.match(/from\s+["']@\/lib\/ansi["']/)) hits.push("引用自研解析模块 @/lib/ansi");
  if (text.match(/\bAnsiText\b/)) hits.push("使用自研渲染器 AnsiText");
  return hits;
}

function signaturesOf(rel: string): string[] {
  const full = path.join(ROOT, rel);
  if (!statSync(full, { throwIfNoEntry: false })?.isFile()) return ["<<文件不存在>>"];
  return parsingSignatures(stripComments(readFileSync(full, "utf8")));
}

/**
 * 允许仍在解析/自研渲染的文件 = **FT4 退役队列**。每条都写清它在干什么，
 * 因为下一条测试会检查"它是不是真的还在干"：不改代码就想把条目留着，会红。
 *
 * 第九十六轮更新：结构化控制台与它的 AnsiText 渲染器已整体退役（连同
 * `FridaConsole.tsx` 与 `frida/types.ts` 的过滤/导出解析），所以那两条从白名单删了；
 * `ansiRuns`（SGR 翻样式）也没了，`ansi.ts` 只剩 stripAnsi 给二进制托管页洗 help 文本。
 */
const ALLOWED_PARSER_FILES: Record<string, string> = {
  "src/lib/ansi.ts": "解析本体（stripAnsi），服务二进制托管页的 help 文本清洗",
  "src/features/binary/BinaryHosting.tsx": "把 help 文本洗成可读段落，不参与终端渲染",
};

describe("护栏一：生产代码不许新增 ANSI/VT 解析", () => {
  const sources = productionSources().map(relative).filter((rel) => rel !== GUARD_FILE);

  it("扫描到了源码清单（清单空了说明这条护栏本身失效了）", () => {
    expect(sources.length).toBeGreaterThan(40);
    expect(sources).toContain("src/lib/terminal.ts");
    expect(sources).toContain("src/lib/ansi.ts");
  });

  it("越界解析的文件必须是零（新增一个就要先回答 §4.1 那条铁律）", () => {
    const offenders: string[] = [];
    for (const rel of sources) {
      if (rel in ALLOWED_PARSER_FILES) continue;
      const hits = signaturesOf(rel);
      if (hits.length > 0) offenders.push(`${rel} ← ${hits.join("、")}`);
    }
    expect(
      offenders,
      `这些文件在自己解析转义序列，而开源内核已经在 src/lib/terminal.ts：\n${offenders.join(
        "\n",
      )}\n要么改用真终端会话，要么在 docs/frida_terminal_phases.md §6.2 里写清"xterm.js 哪里不够"`,
    ).toEqual([]);
  });

  it("白名单里每一条都仍在真实使用（退役了就来把条目删掉）", () => {
    const stale = Object.keys(ALLOWED_PARSER_FILES).filter((rel) => signaturesOf(rel).length === 0);
    expect(stale, `这些条目已经没有对应的解析代码，从白名单里删掉：${stale.join("、")}`).toEqual([]);
  });

  /** 真终端这一侧是新正统：它自己绝不能变成第二个解析器。 */
  it("终端侧文件不含任何解析签名", () => {
    const terminalSide = sources.filter(
      (rel) =>
        rel.startsWith("src/features/terminal/") ||
        [
          "src/lib/terminal.ts",
          "src/lib/terminalSession.ts",
          "src/lib/terminalStore.ts",
          "src/lib/terminalTranscript.ts",
          "src/lib/deviceShell.ts",
          "src/lib/fridaSession.ts",
        ].includes(rel),
    );
    // 5 个 lib + 1 个面板：这条断言钉的是"清单别偷偷变空"，
    // 空了就等于上面那个 for 循环什么都没检查（护栏自己失效比没有护栏更糟）
    expect(terminalSide.length).toBeGreaterThanOrEqual(6);
    for (const rel of terminalSide) {
      expect(signaturesOf(rel), `${rel} 里出现了自研解析`).toEqual([]);
    }
  });
});

describe("护栏二：终端相关依赖只用 xterm.js 家族 + portable-pty", () => {
  interface PackageJson {
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
  }
  const pkg = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8")) as PackageJson;
  const allDeps = { ...pkg.dependencies, ...pkg.devDependencies };

  const ALLOWED_XTERM = [
    "@xterm/xterm",
    "@xterm/addon-fit",
    "@xterm/addon-search",
    "@xterm/addon-serialize",
    "@xterm/addon-unicode-graphemes",
    "@xterm/addon-web-links",
    "@xterm/addon-webgl",
  ];

  it("装了 xterm 内核，且 addon 只用白名单里那七个", () => {
    const xtermDeps = Object.keys(allDeps).filter((name) => name.startsWith("@xterm/"));
    expect(xtermDeps).toContain("@xterm/xterm");
    const unexpected = xtermDeps.filter((name) => !ALLOWED_XTERM.includes(name));
    expect(unexpected, `不在白名单里的终端依赖：${unexpected.join("、")}`).toEqual([]);
  });

  /**
   * `@xterm/addon-canvas` 不是"以后也许能加"，是**核对过不能用**：
   * 0.7.0 的 peer 写着 xterm ^5，v6 线只有 beta。装了它，"webgl 不可用就退 canvas"
   * 那条退路就是个装不上的东西（第七十五轮实测：先探再用，退路是内置 DOM 渲染器）。
   */
  it("明令拒绝的旧内核与不兼容 addon 不许出现", () => {
    const banned = [
      "@xterm/addon-canvas",
      "xterm",
      "xterm-addon-fit",
      "xterm-addon-webgl",
      "hterm",
      "alacritty_terminal",
    ];
    const present = Object.keys(allDeps).filter((name) => banned.includes(name));
    expect(present, `这些终端依赖被明确拒绝（理由见 §6.2 与台账 T0003）：${present.join("、")}`).toEqual([]);
  });

  it("Rust 侧只有 portable-pty，没有第二个终端仿真器", () => {
    const cargo = readFileSync(path.join(ROOT, "src-tauri/Cargo.toml"), "utf8");
    expect(cargo).toMatch(/^portable-pty/m);
    for (const banned of ["alacritty_terminal", "vte", "vt100", "crossterm", "ratatui", "termion"]) {
      expect(cargo, `Cargo.toml 里出现了第二个终端内核/渲染库：${banned}`).not.toContain(banned);
    }
  });
});
