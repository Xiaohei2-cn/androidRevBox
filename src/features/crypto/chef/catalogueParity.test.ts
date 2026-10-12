/**
 * 算法清单与五语词典的**对齐闸门**。
 *
 * 为什么需要它：`chef.op.<id>.name` 这类键是动态拼出来的，`pnpm i18n:check` 只比对
 * 词典之间的键集、看不见代码里的动态键；Rust 注册表加了算法而词典没跟上时，
 * 界面上不会报错，只会安静地显示 `base64Encode` 这种 id（对用户就是"没翻译"）。
 * 所以这里直接从 Rust 源码抽出注册表事实，逐条断言五种语言都有词、且没有孤儿键。
 */
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { DICTIONARIES, type Locale } from "@/i18n";

const ROOT = process.cwd();
const OPS_DIR = "src-tauri/src/services/chef/ops";
const LOCALES: Locale[] = ["zh-CN", "en", "ru", "pt-BR", "ja"];

/**
 * 只取注册表部分：`*_tests.rs` 是纯测试，`#[cfg(test)]` 之后是测试代码，
 * 两者都含有 `op(...)`/`text(...)` 同名调用，混进来会造出一堆假条目。
 */
function rustSources(): { file: string; text: string }[] {
  const dir = path.join(ROOT, OPS_DIR);
  return readdirSync(dir)
    .filter((name) => name.endsWith(".rs") && !name.endsWith("_tests.rs"))
    .map((name) => {
      const raw = readFileSync(path.join(dir, name), "utf8");
      const cut = raw.indexOf("#[cfg(test)]");
      return { file: `${OPS_DIR}/${name}`, text: cut === -1 ? raw : raw.slice(0, cut) };
    });
}

/** `op("id", "category", ...)` 抽算法 id 与分类 id */
function registry(): { ids: string[]; cats: string[]; args: string[] } {
  const ids = new Set<string>();
  const cats = new Set<string>();
  const args = new Set<string>();
  // 排除 args.choice("x") / data.text() 这类同名方法调用：前面不允许出现 . 或单词字符
  const opRe = /(?<![\w.])op\(\s*"([A-Za-z0-9_]+)"\s*,\s*"([a-z-]+)"/g;
  const argRe = /(?<![\w.])(?:text|text_arg|bool_arg|number|choice)\(\s*"([A-Za-z][A-Za-z0-9]*)"/g;
  for (const { text } of rustSources()) {
    for (const m of text.matchAll(opRe)) {
      ids.add(m[1]);
      cats.add(m[2]);
    }
    for (const m of text.matchAll(argRe)) args.add(m[1]);
  }
  return { ids: [...ids], cats: [...cats], args: [...args] };
}

const reg = registry();

describe("chef 注册表解析", () => {
  it("从 Rust 源码里确实抽到了清单（解析失效要立刻暴露，别把空集合当成通过）", () => {
    expect(reg.ids.length).toBeGreaterThanOrEqual(100);
    expect(reg.cats.length).toBeGreaterThanOrEqual(9);
    expect(reg.args.length).toBeGreaterThanOrEqual(80);
    expect(reg.ids).toContain("base64Encode");
    expect(reg.ids).toContain("aesDecrypt");
  });

  it("每张算法表都挂进了 GROUPS，且每个文件只贡献自己那几类", () => {
    const mod = readFileSync(path.join(ROOT, `${OPS_DIR}/mod.rs`), "utf8");
    const tables = rustSources().filter(({ text }) => /pub const OPS/.test(text));
    expect(tables.length).toBeGreaterThanOrEqual(7);
    for (const { file } of tables) {
      const stem = path.basename(file, ".rs");
      expect(mod, `${stem}::OPS 未挂进 GROUPS（清单里不会出现该组的算法）`).toContain(`${stem}::OPS`);
    }
    // 允许 text.rs 附带 misc、digest.rs 附带 checksum；其余文件必须自洽
    const allowed: Record<string, string[]> = { digest: ["digest", "checksum"], text: ["text", "misc"] };
    for (const { file, text } of tables) {
      const stem = path.basename(file, ".rs");
      const used = [...text.matchAll(/(?<![\w.])op\(\s*"[A-Za-z0-9_]+"\s*,\s*"([a-z-]+)"/g)].map((m) => m[1]);
      const expected = allowed[stem] ?? [stem];
      for (const cat of used) expect(expected, `${stem}.rs 里出现了分类 ${cat}`).toContain(cat);
    }
  });
});

describe.each(LOCALES)("算法清单语言对齐 (%s)", (locale) => {
  const dict = DICTIONARIES[locale] as Record<string, string>;

  it("每个算法都有名称", () => {
    const missing = reg.ids.filter((id) => !dict[`chef.op.${id}.name`]?.trim());
    expect(missing, `${locale} 缺算法名: ${missing.join(", ")}`).toEqual([]);
  });

  it("每个分类和参数都有名称", () => {
    const missingCats = reg.cats.filter((cat) => !dict[`chef.category.${cat}`]?.trim());
    expect(missingCats, `${locale} 缺分类名: ${missingCats.join(", ")}`).toEqual([]);
    const missingArgs = reg.args.filter((arg) => !dict[`chef.arg.${arg}`]?.trim());
    expect(missingArgs, `${locale} 缺参数名: ${missingArgs.join(", ")}`).toEqual([]);
  });

  it("不存在词典里有、注册表里没有的算法/分类/参数键（删掉算法要同步删词）", () => {
    const known = new Set(reg.ids);
    const knownCats = new Set(reg.cats);
    const knownArgs = new Set(reg.args);
    const orphans: string[] = [];
    for (const key of Object.keys(dict)) {
      if (key.startsWith("chef.op.")) {
        const id = key.slice("chef.op.".length).replace(/\.name$/, "");
        if (!known.has(id)) orphans.push(key);
      } else if (key.startsWith("chef.category.")) {
        if (!knownCats.has(key.slice("chef.category.".length))) orphans.push(key);
      } else if (key.startsWith("chef.arg.")) {
        if (!knownArgs.has(key.slice("chef.arg.".length))) orphans.push(key);
      }
    }
    expect(orphans, `${locale} 孤儿键: ${orphans.join(", ")}`).toEqual([]);
  });

  it("简体中文不能把算法名留成英文 id（清单必须真的本地化）", () => {
    const rawIds = reg.ids.filter((id) => dict[`chef.op.${id}.name`] === id);
    expect(rawIds, `${locale} 直接显示 id 的条目: ${rawIds.join(", ")}`).toEqual([]);
  });
});

describe("chef 词典跨语言一致性", () => {
  it("五种语言的 chef.* 键集完全相同", () => {
    const sets = LOCALES.map(
      (locale) =>
        new Set(
          Object.keys(DICTIONARIES[locale] as Record<string, string>).filter((key) => key.startsWith("chef.")),
        ),
    );
    const [first, ...rest] = sets;
    for (const [index, set] of rest.entries()) {
      const missing = [...first].filter((key) => !set.has(key));
      const extra = [...set].filter((key) => !first.has(key));
      expect(missing, `${LOCALES[index + 1]} 缺 ${missing.slice(0, 10).join(", ")}`).toEqual([]);
      expect(extra, `${LOCALES[index + 1]} 多 ${extra.slice(0, 10).join(", ")}`).toEqual([]);
    }
  });
});

describe("chef 界面文案键", () => {
  const sources = [
    ...readdirSync(path.join(ROOT, "src/features/crypto/chef"))
      .filter((name) => name.endsWith(".tsx"))
      .map((name) => readFileSync(path.join(ROOT, "src/features/crypto/chef", name), "utf8")),
    readFileSync(path.join(ROOT, "src/features/crypto/CryptoPage.tsx"), "utf8"),
  ].join("\n");

  /** 代码里用到的键：labels.ui("x") -> chef.ui.x，title("x") -> chef.ui.step.x，t("chef.x") 原样 */
  function usedKeys(): string[] {
    const keys = new Set<string>();
    for (const m of sources.matchAll(/ui\("([A-Za-z.]+)"/g)) keys.add(`chef.ui.${m[1]}`);
    for (const m of sources.matchAll(/title\("([A-Za-z]+)"/g)) keys.add(`chef.ui.step.${m[1]}`);
    for (const m of sources.matchAll(/\bt\("(chef\.[A-Za-z0-9_.]+)"/g)) keys.add(m[1]);
    return [...keys];
  }

  it("用到的外壳文案在每种语言里都存在", () => {
    const used = usedKeys();
    expect(used.length).toBeGreaterThan(30);
    for (const locale of LOCALES) {
      const dict = DICTIONARIES[locale] as Record<string, string>;
      const missing = used.filter((key) => !dict[key]?.trim());
      expect(missing, `${locale} 缺界面文案: ${missing.join(", ")}`).toEqual([]);
    }
  });
});
