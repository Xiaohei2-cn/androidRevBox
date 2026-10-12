/**
 * 窗口底色的渐变闸门（只读 globals.css，不渲染）。
 *
 * 为什么要有这道闸：这轮之前渐变"画了但看不出来"——两端只差 7% 亮度，
 * 在半透明窗口上等于还是纯色，而代码看起来完全正常。
 * 所以这里不看"有没有 gradient"，只看**跨度**：跨度掉下去就红。
 */
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(path.join(process.cwd(), "src/styles/globals.css"), "utf8");

function stopsFor(block: string): Record<"hi" | "mid" | "lo", number[]> {
  const out = {} as Record<"hi" | "mid" | "lo", number[]>;
  for (const key of ["hi", "mid", "lo"] as const) {
    const m = block.match(new RegExp("--app-bg-" + key + ":\\s*(\\d+) (\\d+) (\\d+);"));
    expect(m, "找不到 --app-bg-" + key).toBeTruthy();
    out[key] = m![1].split(" ").map(Number);
  }
  return out;
}

function themeBlocks(): { name: string; block: string }[] {
  const dark = css.slice(css.indexOf(".dark {"));
  const light = css.slice(0, css.indexOf(".dark {"));
  return [
    { name: "light", block: light },
    { name: "dark", block: dark },
  ];
}

describe("窗口底色是渐变，而且必须看得出是渐变", () => {
  it.each(themeBlocks())("$name 三档色的跨度足够（不许退化成纯色）", ({ name, block }) => {
    const { hi, lo } = stopsFor(block);
    const spread = Math.max(...hi.map((v, i) => Math.abs(v - lo[i])));
    expect(spread, name + " 的 hi/lo 只差 " + spread + "，肉眼读不出渐变").toBeGreaterThanOrEqual(24);
  });

  it("透明度整体撤掉：alpha 恒为 1，React 侧不留出口（三档与光斑仍乘同一个变量）", () => {
    const rule = css.match(/\.app-surface\s*\{([^}]*)\}/)![1];
    const linear = rule.slice(rule.lastIndexOf("linear-gradient"));
    for (const key of ["hi", "mid", "lo"]) {
      expect(rule).toContain("rgb(var(--app-bg-" + key + ") / var(--app-bg-alpha))");
    }
    expect(linear).toContain("--app-bg-alpha");
    // 光斑与底色吃同一个 alpha：万一以后有人把它调回半透明，角落不会比中间更"实"
    expect(rule).toContain("calc(var(--app-bg-tint-alpha) * var(--app-bg-alpha))");
    expect(rule).toContain("calc(var(--app-bg-tint2-alpha) * var(--app-bg-alpha))");
    // 只许定义一处，而且就是 1：窗口底色不透视，没有"第二处把 alpha 改小"的余地
    const defined = [...css.matchAll(/--app-bg-alpha:\s*([^;]+);/g)].map((m) => m[1].trim());
    expect(defined, "alpha 只允许一处定义，实际 " + JSON.stringify(defined)).toEqual(["1"]);
    // React 不再写它：背景透明度既没有滑杆，也没有设置键
    for (const file of ["src/app/AppShell.tsx", "src/app/providers.tsx", "src/features/settings/SettingsPage.tsx"]) {
      const source = readFileSync(path.join(process.cwd(), file), "utf8");
      expect(source, file + " 不该再把 alpha 当样式键写下去").not.toContain('"--app-bg-alpha"');
      expect(source, file + " 里不该再有 setOpacity").not.toContain("setOpacity");
    }
  });

  it("两枚对角光斑 + 一条斜向色阶：单枚光斑看着像一边脏了", () => {
    const rule = css.match(/\.app-surface\s*\{([^}]*)\}/)![1];
    expect((rule.match(/radial-gradient/g) ?? []).length).toBe(2);
    expect((rule.match(/linear-gradient/g) ?? []).length).toBe(1);
    expect(rule).toContain("at 6% 0%");
    expect(rule).toContain("at 96% 100%");
    // 齿块/主体/控制栏三个元素要拼成同一片色阶，靠 fixed 按整窗坐标铺
    expect(rule).toContain("background-attachment: fixed");
  });

  it("控制栏同一条色阶但恒为实色（不吃滑杆）", () => {
    const rule = css.match(/\.app-titlebar\s*\{([^}]*)\}/)![1];
    expect(rule).not.toContain("--app-bg-alpha");
    for (const key of ["hi", "mid", "lo"]) {
      expect(rule).toContain("rgb(var(--app-bg-" + key + ") / 1)");
    }
  });

  it("内容区不许再盖一层厚灰纱（那层会把渐变压平）", () => {
    const rule = css.match(/\.page-canvas\s*\{([^}]*)\}/)![1];
    const alphas = [...rule.matchAll(/\/\s*([0-9.]+)\)/g)].map((m) => Number(m[1]));
    expect(Math.max(...alphas), "page-canvas 叠色最重处 " + Math.max(...alphas)).toBeLessThanOrEqual(0.03);
  });
});

/**
 * 这一类失误必须被机器挡住：`.app-surface` 现在带 `background-image`（窗口渐变），
 * 而 Tailwind 的 `bg-primary` / `hover:bg-accent` 只写 `background-color`——
 * 不清掉图像层的话，渐变会盖在组件自己的颜色上面。
 * 实际踩过一次：选中齿块因此变成"白字压在近白底上"（对比度 1.0:1），
 * 代码看着完全正常、CSS 也确实生效了，只有肉眼能发现。
 *
 * 规则：**同一个 cn() 分支字符串里**只要写了 `bg-<色>`，就必须同时写 `bg-none`。
 * 粒度要落在单个字符串上——探针试过按"元素附近 420 字符"开窗，
 * 结果未选中分支的 `hover:bg-none` 替选中分支背了锅，闸门直接失效。
 */
const BG_COLOR = /\bbg-(primary|secondary|card|popover|accent|muted|background|destructive)\b/;
const LITERAL = /"([^"\n]*)"/g;

function listSurfaceFiles(): string[] {
  const dirs = ["src/components/nav", "src/components/window", "src/app", "src/features"];
  const out: string[] = [];
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith(".tsx") && !entry.name.includes(".test.")) out.push(full);
    }
  };
  for (const d of dirs) walk(path.join(process.cwd(), d));
  return out;
}

/** 取出一段 cn(...) 调用里的所有分支字符串 */
function cnCallLiterals(text: string, from: number): string[] {
  const open = text.indexOf("(", from);
  if (open === -1) return [];
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === "(") depth += 1;
    else if (text[i] === ")") {
      depth -= 1;
      if (depth === 0) return [...text.slice(open, i).matchAll(LITERAL)].map((m) => m[1]);
    }
  }
  return [];
}

function offenders(source: string): string[] {
  const bad: string[] = [];
  for (const m of source.matchAll(/\bcn\(/g)) {
    const literals = cnCallLiterals(source, m.index!);
    if (!literals.some((l) => l.includes("app-surface"))) continue;
    for (const literal of literals) {
      if (BG_COLOR.test(literal) && !literal.includes("bg-none")) bad.push(literal.trim());
    }
  }
  return bad;
}

describe("app-surface 不许吞掉组件自己的背景色", () => {
  it("闸门自己要有牙：故意漏掉 bg-none 的样本必须被判违规", () => {
    const sample = 'className={cn("app-surface flex", active ? "bg-primary text-white" : "w-9")}';
    expect(offenders(sample)).toHaveLength(1);
    const fixed = 'className={cn("app-surface flex", active ? "bg-none bg-primary" : "w-9")}';
    expect(offenders(fixed)).toHaveLength(0);
  });

  it.each(listSurfaceFiles().map((f) => [f.replace(process.cwd() + "/", "")]))(
    "%s 里叠了颜色的分支都清了图像层",
    (rel) => {
      let text = readFileSync(path.join(process.cwd(), rel), "utf8");
      text = text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
      expect(offenders(text), `${rel}: app-surface + bg-<色> 却没写 bg-none（渐变会把颜色盖掉）`).toEqual([]);
    },
  );
});
