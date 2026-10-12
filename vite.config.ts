import { defineConfig, type Plugin } from "vitest/config";
import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

/**
 * frida 类型库的原样导入（Monaco IDE 注入用，见
 * `src/features/hook/frida/ide/monacoSetup.ts`）。
 *
 * 这两个虚拟模块把 node_modules 里的 .d.ts 以**字符串**形式交给前端：
 * - 为什么不用 `?raw` 后缀直接导入：TS 会把以 `.d.ts` 结尾的导入当类型声明文件
 *   而拒绝（TS6137）；把 specifier 换成 `@frida-types/*` 别名后 TS 那一关过了，
 *   但 Vite 的依赖预优化器又不认识别名带查询串的形状（dev 下报
 *   "optimized info should be defined"、动态导入直接失败）。
 * - 所以走 `resolveId`/`load` 显式提供虚拟模块：dev 与 build 都只有一条路径，
 *   不再依赖预优化与别名的配合。`?raw` 名字保留在 specifier 里只是给人看的。
 */
function fridaTypesRaw(): Plugin {
  const VIRTUAL: Record<string, string> = {
    "@frida-types/gum?raw": "./node_modules/@types/frida-gum/index.d.ts",
    "@frida-types/java-bridge?raw": "./node_modules/frida-java-bridge/index.d.ts",
  };
  return {
    name: "frida-types-raw",
    resolveId(source) {
      if (source in VIRTUAL) return `\0${source}`;
      return null;
    },
    load(id) {
      if (!id.startsWith("\0@frida-types/")) return null;
      const key = id.slice(1);
      const rel = VIRTUAL[key];
      if (!rel) return null;
      const content = readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
      // 以模块形式导出字符串（Vite 会把它包成 ESM）
      return `export default ${JSON.stringify(content)};`;
    },
  };
}

export default defineConfig({
  plugins: [react(), fridaTypesRaw()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./vitest.setup.ts"],
    globals: false,
    css: false,
  },
});