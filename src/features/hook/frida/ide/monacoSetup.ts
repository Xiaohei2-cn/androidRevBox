/**
 * Monaco 装载与 frida 类型库注入（IDE 的"智能提示"就从这里来）。
 *
 * 与 `frida-agent-example` 的做法同源：PyCharm/VS Code 里能提示 frida API，
 * 靠的不是什么魔法，而是项目挂了 `@types/frida-gum`（agent 运行时全量全局 API 的
 * d.ts）与 `frida-java-bridge`（`Java.*` 桥）。Monaco 自带 TS 语言服务，
 * `addExtraLib` 把这两份声明注进去，`.js` 脚本上就有：
 * - `Interceptor.` → attach/detach/replace…（带签名与 JSDoc 悬停）
 * - `Memory.alloc(16)` 之后 `p.` 能接着提示（真类型推导，不是关键词表）
 * - `Java.perform(...)` 参数提示
 *
 * 装载策略（对 0.57 的实测形状）：
 * - 编辑器壳体从 `monaco-editor/editor/editor.api` 进（不带任何语言定义），
 *   js 基础高亮用 `languages/definitions/javascript/register` 注册；
 * - TS 语言服务与 worker 走 `languages/features/typescript/register`；
 * - `?worker` / `?raw` 后缀由 Vite 处理；本模块只被 lazy 的 FridaIde 引用，
 *   所以 3.7MB 编辑器主体 + 6.7MB 语言服务 worker 都不进首屏主块。
 */

import * as monaco from "monaco-editor/editor/editor.api";
/** Monaco 模块类型（editor.api 的模块身份）——ScriptIde 的 ref 用它，别用 monaco-editor 主入口 */
export type MonacoModule = typeof monaco;
import "monaco-editor/languages/definitions/javascript/register";
// TS/JS 语言服务（0.57 的顶层命名空间；`languages.typescript` 已弃用）
import {
  javascriptDefaults,
  typescriptDefaults,
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
} from "monaco-editor/languages/features/typescript/register";

// 通用 editor worker（单词建议、链接检测等基础能力）
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
// TS/JS 语言服务 worker（补全、悬停、诊断的运算都在这里面）
import TsWorker from "monaco-editor/languages/features/typescript/ts.worker?worker";

// ===== frida 类型库（原样注入，来源见文件头） =====
// `?raw` = Vite 构建期把整个 .d.ts 作为字符串打进产物；只在 IDE 块里。
// 两个 specifier 走 vite.config 里的 `@frida-types/*` 别名（别直接写 `xx.d.ts?raw`：
// TS 会把以 `.d.ts` 结尾的导入当类型声明文件拒绝，报 TS6137）。
import fridaGumDts from "@frida-types/gum?raw";
import fridaJavaBridgeDts from "@frida-types/java-bridge?raw";

const FRIDA_GUM_LIB = "file:///frida/@types/frida-gum/index.d.ts";
const FRIDA_JAVA_LIB = "file:///frida/frida-java-bridge/index.d.ts";
const FRIDA_SHIMS_LIB = "file:///frida/frida-shims.d.ts";

/**
 * 两件 frida 脚本里天天用、但不在上面两份 d.ts 里的东西，补一个小垫片：
 * - **全局 `Java`**：`frida-java-bridge/index.d.ts` 只提供
 *   `declare module "frida-java-bridge"`，而 REPL 里用的是全局 `Java`。
 *   从模块声明的 default 导出把类型引出来，不手抄第二份（抄的那份必然跟版本漂移）。
 * - **`console`**：它本来在 `lib.dom.d.ts` 里，而我们的 lib 集合**故意不含 DOM**
 *   （frida-gum 自己声明了 `Worker`/`File`，加 DOM 会撞 duplicate identifier）。
 */
const fridaShims = `
declare const Java: typeof import("frida-java-bridge").default;
declare const console: {
  log(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
};
`;

let configured = false;

/** 每个 label 只启一个 Worker：同 label 复用，别把进程开成一排 */
const workers = new Map<string, Worker>();

/**
 * 一次性配置：worker 工厂 + 语言服务开关 + frida 类型库。
 * 幂等（守着一个模块级布尔），重复调用不会把 extraLib 叠两份。
 */
export function setupMonaco(): typeof monaco {
  if (configured) return monaco;
  configured = true;

  // worker 工厂：ts/js 走 TS worker，其余走通用 editor worker
  (globalThis as { MonacoEnvironment?: monaco.Environment }).MonacoEnvironment = {
    getWorker: (_workerId: string, label: string) => {
      const cached = workers.get(label);
      if (cached) return cached;
      const worker =
        label === "typescript" || label === "javascript" ? new TsWorker() : new EditorWorker();
      workers.set(label, worker);
      return worker;
    },
  };

  // 用户写的是**给 frida 内核直接 eval 的脚本**（不是模块）：CommonJS 会让
  // import/require 报红，而 `require("frida-java-bridge")` 只在 frida-compile
  // 打包时有效；这里给脚本本身选 None + Classic 解析。
  const compilerOptions = {
    target: ScriptTarget.ESNext,
    module: ModuleKind.None,
    allowNonTsExtensions: true,
    allowJs: true,
    // 故意开 checkJs：真实拼写错误（Interceptor.attch）当场画波浪线；
    // frida 惯用法（send/recv/Java.*）在注入的 d.ts 里都有，不会被误报。
    checkJs: true,
    moduleResolution: ModuleResolutionKind.Classic,
    // **不含 DOM**（与 frida-agent-example 的 tsconfig 同一理由）：frida-gum 的 d.ts
    // 自己声明了 `Worker`/`File`，加上 DOM lib 会撞出 duplicate identifier 满屏红。
    lib: ["ES2022"],
    noEmit: true,
    strict: false,
  };
  typescriptDefaults.setCompilerOptions(compilerOptions as never);
  javascriptDefaults.setCompilerOptions(compilerOptions as never);

  // 语法诊断两侧都开（写错括号当场可见）；语义诊断：
  // - TS 侧开（悬停/补全背后的类型推导结果要正确画出来）；
  // - JS 侧关——frida 脚本里大量"运行时才存在"的名字，lib 集合配不齐，开着只会满屏红。
  typescriptDefaults.setDiagnosticsOptions({ noSemanticValidation: false, noSyntaxValidation: false });
  javascriptDefaults.setDiagnosticsOptions({
    noSemanticValidation: true,
    noSyntaxValidation: false,
  });

  // frida 全量 API 与 Java 桥的类型声明（这就是"提示能提示 frida 代码"的来源）
  for (const defaults of [typescriptDefaults, javascriptDefaults]) {
    defaults.addExtraLib(fridaGumDts, FRIDA_GUM_LIB);
    defaults.addExtraLib(fridaJavaBridgeDts, FRIDA_JAVA_LIB);
    defaults.addExtraLib(fridaShims, FRIDA_SHIMS_LIB);
  }

  return monaco;
}

/** Monaco 主题 id 前缀（defineTheme/editor.create 都用它） */
export const IDE_THEME = "app-reverse-frida";

let themeDefined = false;

/**
 * 定义 IDE 双主题并返回要用的那个 id。颜色取自 globals.css 的 shadcn 令牌
 * （暗色 card `235 10% 20%` ≈ #2e2f38、border `235 8% 29%` ≈ #444550，
 * 主色 `243 75% 59%` ≈ #5048e5），不另造色板。
 */
export function ensureIdeTheme(dark: boolean): string {
  const name = dark ? `${IDE_THEME}-dark` : `${IDE_THEME}-light`;
  if (themeDefined) return name;
  themeDefined = true;

  monaco.editor.defineTheme(`${IDE_THEME}-dark`, {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "8b93a7" },
      { token: "keyword", foreground: "c586c0" },
      { token: "string", foreground: "ce9178" },
      { token: "number", foreground: "b5cea8" },
      { token: "type", foreground: "4ec9b0" },
      { token: "function", foreground: "dcdcaa" },
    ],
    colors: {
      "editor.background": "#2e2f38",
      "editor.foreground": "#eaeaef",
      "editorLineNumber.foreground": "#6b7280",
      "editorLineNumber.activeForeground": "#d1d5db",
      "editor.lineHighlightBackground": "#ffffff0a",
      "editor.selectionBackground": "#5048e566",
      "editorCursor.foreground": "#818cf8",
      "editorIndentGuide.background1": "#ffffff14",
      "editorWidget.background": "#3a3b45",
      "editorWidget.border": "#444550",
      "editorSuggestWidget.background": "#3a3b45",
      "editorSuggestWidget.selectedBackground": "#5048e566",
      "editorHoverWidget.background": "#3a3b45",
      "editorHoverWidget.border": "#444550",
      "editorBracketMatch.background": "#5048e533",
      "editorBracketMatch.border": "#818cf8",
      "editorOverviewRuler.border": "#00000000",
    },
  });
  monaco.editor.defineTheme(`${IDE_THEME}-light`, {
    base: "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6b7280" },
      { token: "keyword", foreground: "7c3aed" },
      { token: "string", foreground: "b45309" },
      { token: "number", foreground: "15803d" },
      { token: "type", foreground: "0f766e" },
      { token: "function", foreground: "1d4ed8" },
    ],
    colors: {
      "editor.background": "#ffffff",
      "editor.lineHighlightBackground": "#f1f5f9",
      "editor.selectionBackground": "#5048e533",
      "editorWidget.background": "#f8fafc",
      "editorWidget.border": "#e2e8f0",
      "editorSuggestWidget.selectedBackground": "#5048e522",
      "editorHoverWidget.background": "#f8fafc",
      "editorHoverWidget.border": "#e2e8f0",
      "editorBracketMatch.background": "#5048e522",
      "editorBracketMatch.border": "#5048e5",
    },
  });
  return name;
}