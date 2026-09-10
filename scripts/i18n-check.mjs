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

process.exit(failed ? 1 : 0);
