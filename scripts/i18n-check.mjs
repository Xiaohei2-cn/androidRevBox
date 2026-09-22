#!/usr/bin/env node
/**
 * i18n 词典完整性校验（P8）。
 * 规则：zh-CN 为源语言必须最全；其余语言键集必须与 zh-CN 完全一致
 * （多、少都算失败）——保证「新增功能必须同步补多语词典」落到 CI。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
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
