#!/usr/bin/env node
/**
 * i18n 词典完整性校验（P8）。
 * 规则：zh-CN 为源语言必须最全；其余语言键集必须与 zh-CN 完全一致
 * （多、少都算失败）——保证「新增功能必须同步补多语词典」落到 CI。
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(fileURLToPath(new URL(".", import.meta.url)), "..");
const root = join(repoRoot, "src/i18n/locales");
const KEY_RE = /"([a-z]+\.[A-Za-z0-9_.]+)":/g;

function collectKeys(lang) {
  const keys = new Set();
  const dir = join(root, lang);
  for (const f of readdirSync(dir)) {
    if (!f.endsWith(".ts") || f === "index.ts") continue;
    const text = readFileSync(join(dir, f), "utf8");
    for (const m of text.matchAll(KEY_RE)) keys.add(m[1]);
  }
  return keys;
}

function listLangs() {
  return readdirSync(root).filter((n) => statSync(join(root, n)).isDirectory());
}

const langs = listLangs();
const source = "zh-CN";
if (!langs.includes(source)) {
  console.error(`源语言目录缺失: ${source}`);
  process.exit(1);
}
const zh = collectKeys(source);
let failed = false;

for (const lang of langs.filter((l) => l !== source)) {
  const keys = collectKeys(lang);
  const missing = [...zh].filter((k) => !keys.has(k));
  const extra = [...keys].filter((k) => !zh.has(k));
  if (missing.length || extra.length) {
    failed = true;
    console.error(`✗ ${lang}: 缺 ${missing.length} 键, 多 ${extra.length} 键`);
    for (const k of missing) console.error(`   missing: ${k}`);
    for (const k of extra) console.error(`   extra:   ${k}`);
  } else {
    console.log(`✓ ${lang}: ${keys.size} 键与 zh-CN 对齐`);
  }
}
console.log(`zh-CN: ${zh.size} 键（源语言）`);

// Rust 白名单同步检查：ConfigService::LOCALES 必须包含前端全部语言
try {
  const rs = readFileSync(
    join(repoRoot, "src-tauri/src/services/config_service.rs"),
    "utf8",
  );
  for (const lang of langs) {
    if (!rs.includes(`"${lang}"`)) {
      failed = true;
      console.error(`✗ Rust ConfigService::LOCALES 缺少语言 "${lang}"（两处必须同时登记）`);
    }
  }
} catch {
  console.warn("（跳过 Rust 白名单检查：config_service.rs 不可读）");
}


// 反向校验：代码里写死的 t("域.键") 必须真的存在于源语言词典。
// `t` 的键不是类型化的（见 src/i18n/context.tsx），拼错只会静默回退成键名显示在界面上，
// 所以这里补一道：新增文案改了键名却没同步改词典，CI 直接红。
function collectUsedKeys() {
  const used = new Map(); // key -> 出现位置（取第一个，便于直接跳过去看）
  const srcDir = join(repoRoot, "src");
  const skipFile = (name) =>
    name.endsWith(".test.ts") || name.endsWith(".test.tsx") || name.includes("i18n-check");
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === "node_modules" || entry.name === "locales") continue;
        walk(full);
        continue;
      }
      if (!/\.(ts|tsx)$/.test(entry.name) || skipFile(entry.name)) continue;
      const text = readFileSync(full, "utf8");
      for (const m of text.matchAll(/\bt\(\s*"([a-z]+\.[A-Za-z0-9_.]+)"/g)) {
        // 跳过注释行：文档里的 t("域.键") 是写法示例，不是真的取值
        const lineStart = text.lastIndexOf("\n", m.index) + 1;
        const head = text.slice(lineStart, m.index).trimStart();
        if (head.startsWith("//") || head.startsWith("*") || head.startsWith("/*")) continue;
        if (!used.has(m[1])) used.set(m[1], full.replace(repoRoot + "/", ""));
      }
    }
  };
  walk(srcDir);
  return used;
}


// ---------------------------------------------------------------------------
// 第三道闸：写死在组件里的中文（这些不会被上面两道闸看见）。
//
// 为什么要管：本仓库支持 5 种语言，但历史上有些页面（任务中心整页、设备页若干空态、
// 控制栏的 aria 名）把文案直接写成了中文字面量——切到英文/俄语时这些地方仍是中文，
// 用户看到的就是"翻译做了一半"。
//
// 现状还很多，所以这里做**棘轮**而不是立刻清零：
// 基线 scripts/i18n-hardcoded-baseline.json 记每个文件当前的行数，
// **只许减不许增**；某个文件降了，就把基线改小（脚本会提示你）。
// ---------------------------------------------------------------------------
const CJK_RE = /[\u4e00-\u9fff]/;
const BASELINE_FILE = join(repoRoot, "scripts/i18n-hardcoded-baseline.json");
const baseline = existsSync(BASELINE_FILE)
  ? JSON.parse(readFileSync(BASELINE_FILE, "utf8"))
  : {};

function countHardcodedCjk() {
  const found = {};
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      if (statSync(full).isDirectory()) {
        if (name === "i18n") continue; // 词典本身当然全是目标语言
        walk(full);
        continue;
      }
      if (!name.endsWith(".tsx") || name.includes(".test.")) continue;
      const text = readFileSync(full, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      let n = 0;
      for (const line of text.split("\n")) {
        const t = line.trim();
        if (t.startsWith("//") || t.startsWith("*") || t.startsWith("/*")) continue;
        // 开发者日志不算界面文案（console.* 里带中文不该挡 CI）
        if (line.includes("console.")) continue;
        if (CJK_RE.test(line)) n += 1;
      }
      const rel = full.replace(repoRoot + "/", "");
      if (n > 0) found[rel] = n;
    }
  };
  walk(join(repoRoot, "src"));
  return found;
}

const hardcoded = countHardcodedCjk();
const over = [];
const improved = [];
for (const [file, n] of Object.entries(hardcoded)) {
  const allowed = baseline[file] ?? 0;
  if (n > allowed) over.push([file, n, allowed]);
  else if (n < allowed) improved.push([file, n, allowed]);
}
if (over.length) {
  failed = true;
  console.error(`\u2717 有 ${over.length} 个文件新增了写死的中文界面文案（棘轮只许减不许增）`);
  for (const [file, n, allowed] of over) {
    console.error(`   ${file}: 现在 ${n} 行，基线 ${allowed} 行 -> 请改用 t("域.键") 并补 5 份词典`);
  }
} else if (improved.length) {
  const totalNow = Object.values(hardcoded).reduce((a, b) => a + b, 0);
  console.log(`\u26a0 写死中文比基线少了 ${improved.length} 个文件（现在共 ${totalNow} 行）：记得把 scripts/i18n-hardcoded-baseline.json 改小，别让它反弹`);
  for (const [file, n, allowed] of improved) console.log(`   ${file}: ${allowed} -> ${n}`);
} else {
  const totalNow = Object.values(hardcoded).reduce((a, b) => a + b, 0);
  console.log(`\u2713 写死中文未新增（棘轮基线内，当前合计 ${totalNow} 行待收进词典）`);
}

const used = collectUsedKeys();
const unknown = [...used].filter(([key]) => !zh.has(key));
if (unknown.length) {
  failed = true;
  console.error(`✗ 代码里用了 ${unknown.length} 个词典中不存在的键（界面会直接显示键名）`);
  for (const [key, where] of unknown) console.error(`   ${key}  ← ${where}`);
} else {
  console.log(`✓ 代码内 ${used.size} 个字面量键全部在 zh-CN 词典中存在`);
}

process.exit(failed ? 1 : 0);
