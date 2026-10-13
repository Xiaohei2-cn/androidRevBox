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

// ⚠️ `editor.api` 是**纯 API 工厂**（只 6 个 import，不含任何编辑器功能）。补全菜单、
// 折叠、查找、括号配对…全都是"贡献点（contrib）"，必须自己引——不引的后果不是报错，
// 而是**静默失效**：TS worker 明明算得出补全（实测 `Interceptor.` 回 8 项），
// 但屏上永远不弹建议控件，因为订阅/显示它的 suggestController 根本没进包。
// 下面这一组是"轻量 IDE"真正要用的最小值（每个都对着界面上一个可见能力）。
import "monaco-editor/editor/browser/coreCommands"; // 光标/选择等核心命令
import "monaco-editor/editor/contrib/suggest/browser/suggestController"; // **补全菜单本体**
import "monaco-editor/editor/contrib/suggest/browser/suggestInlineCompletions";
import "monaco-editor/editor/contrib/hover/browser/hoverContribution"; // 悬停文档（类型说明）
import "monaco-editor/editor/contrib/parameterHints/browser/parameterHints"; // 参数提示
import "monaco-editor/editor/contrib/folding/browser/folding"; // 代码折叠
import "monaco-editor/editor/contrib/bracketMatching/browser/bracketMatching"; // 括号配对
import "monaco-editor/editor/contrib/find/browser/findController"; // Ctrl+F 查找
import "monaco-editor/editor/contrib/comment/browser/comment"; // Ctrl+/ 注释
import "monaco-editor/editor/contrib/wordOperations/browser/wordOperations"; // 按词移动/删除
import "monaco-editor/editor/contrib/linesOperations/browser/linesOperations"; // 行操作
import "monaco-editor/editor/contrib/indentation/browser/indentation"; // 缩进
import "monaco-editor/editor/contrib/clipboard/browser/clipboard"; // 复制/粘贴
import "monaco-editor/editor/contrib/multicursor/browser/multicursor"; // 多光标
import "monaco-editor/editor/contrib/cursorUndo/browser/cursorUndo";
import "monaco-editor/editor/contrib/contextmenu/browser/contextmenu"; // 右键菜单
import "monaco-editor/editor/contrib/format/browser/formatActions";
// js 基础高亮（语法着色）——只这一个语言，别把 81 种全拖进来
import "monaco-editor/languages/definitions/javascript/register";
// TS/JS 语言服务（0.57 的顶层命名空间；`languages.typescript` 已弃用）
import {
  javascriptDefaults,
  typescriptDefaults,
  ModuleKind,
  ModuleResolutionKind,
  ScriptTarget,
} from "monaco-editor/languages/features/typescript/register";

import { registerFridaCallbackCompletions } from "./fridaCallbacks";

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
const FRIDA_JAVA_SHIM_LIB = "file:///frida/frida-java-global.d.ts";

/**
 * `console` 的垫片（**恒注**）：它本来在 `lib.dom.d.ts` 里，而我们的 lib 集合
 * **故意不含 DOM**（frida-gum 自己声明了 `Worker`/`File`，加 DOM 会撞 duplicate
 * identifier）。frida 脚本天天 `console.log`，所以这一条任何情况下都要有。
 */
const CONSOLE_SHIM = `
declare const console: {
  log(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
};
`;

/**
 * 全局 `Java` 的垫片（**按需注入**，见 `injectFridaLibs`）。
 *
 * 为什么要按需：`Java` 命名空间有两个可能的来源——
 * - **@types/frida-gum 18.x 及更早自带** `declare namespace Java`（用户 PyCharm 那份
 *   18.8.2 就是这样，第 5376 行），那种情况下它就是"正统"，再注一份等于重复声明；
 * - gum 19.x **删掉了**它，此后 `Java` 只能从 `frida-java-bridge` 的类型库来。
 * 两边都注会出现同一命名空间声明两次——语言服务不一定报错，但结论可能诡异
 * （这正是"提示看着不太一样"的一种来源）。所以：**谁有就听谁的，不叠加**。
 */
const JAVA_FROM_JB_SHIM = `
declare const Java: typeof import("frida-java-bridge").default;
`;

/** 一份类型库的来源描述（决定垫片怎么注） */
interface FridaTypeSet {
  gum: string;
  javaBridge: string | null;
  /** gum 是否自带 Java 命名空间（自带则不再注 java-bridge 的 Java） */
  gumHasJava: boolean;
}

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

  // frida 全量 API 与 Java 桥的类型声明（这就是"提示能提示 frida 代码"的来源）。
  // 默认先注**内置版本**，保证"打开就有提示"；选定环境解析到别的版本后，
  // 由 applyFridaTypes() 覆盖（见下）。
  //
  /*
   * ⚠️ 只在"还没有更好的"时注内置兜底——**绝不许覆盖已经注入好的那份**。
   *
   * 真实事故（app 里才复现，预览里因为时序不同而掩盖了）：
   * `ScriptIde` 的 effect 先跑 `applyFridaTypes()` 注入了用户的 18.8.2；
   * 用户随后点开编辑器 → `setupMonaco()` 首次执行 → 这里无条件注内置 →
   * **把 18.8.2 覆盖回内置 19.x**，连 appliedKey 也被改回内置指纹。
   * 于是"状态条说 18.8.2、补全却是 19.x 的 getGlobalExportByName"。
   *
   * 判据用 appliedKey：它非 null 就说明**已有一份按环境解析过的库**在生效，
   * 内置兜底此时既不该注入、更不该改指纹。
   */
  if (appliedKey === null) {
    injectFridaLibs(BUNDLED_FRIDA_TYPES, false);
    appliedKey = typeSetKey(BUNDLED_FRIDA_TYPES);
  }

  // 补 TS 语言服务不做的那些 union 回调成员补全（详见 fridaCallbacks.ts 的文件头）
  registerFridaCallbackCompletions(monaco);

  return monaco;
}

/** 一份类型库的指纹（判断"是不是换了"） */
function typeSetKey(libs: FridaTypeSet): string {
  return `${libs.gum.length}|${libs.javaBridge?.length ?? 0}|${libs.gumHasJava}`;
}

/** 已注入的类型库句柄（重复注入时先 dispose 旧的，避免 extraLib 叠加出重复声明） */
let injectedLibs: { dispose(): void }[] = [];

/**
 * 注入一套类型库 + 必要的垫片。**垫片按来源决定**（这是修"重复声明"的关键）：
 * - `console`：恒注（frida 脚本天天用，且我们的 lib 不含 DOM）。
 * - 全局 `Java`：只有当 **gum 不自带**、且**给了 java-bridge** 时才注 java-bridge 版；
 *   否则交给 gum 自带的 `declare namespace Java`（18.x 及更早），不叠加。
 */
function injectFridaLibs(libs: FridaTypeSet, replaceWorker: boolean): void {
  for (const d of injectedLibs) d.dispose();
  injectedLibs = [];
  const javaFromGum = libs.gumHasJava;
  const canBridgeJava = !javaFromGum && !!libs.javaBridge;
  for (const defaults of [typescriptDefaults, javascriptDefaults]) {
    injectedLibs.push(defaults.addExtraLib(libs.gum, FRIDA_GUM_LIB));
    if (libs.javaBridge) {
      injectedLibs.push(defaults.addExtraLib(libs.javaBridge, FRIDA_JAVA_LIB));
    }
    if (canBridgeJava) {
      injectedLibs.push(defaults.addExtraLib(JAVA_FROM_JB_SHIM, FRIDA_JAVA_SHIM_LIB));
    }
    injectedLibs.push(defaults.addExtraLib(CONSOLE_SHIM, FRIDA_SHIMS_LIB));
  }
  if (replaceWorker) {
    recycleTsWorker();
    // 重建之后 worker 会在下次取用时按**新**库冷启，这里记下它是按哪套建的
    workerLibKey = typeSetKey(libs);
  }
}

/**
 * 丢掉已启动的 TS worker，让语言服务**从干净状态重新加载**（含新注的 extraLib）。
 *
 * 为什么必须这么做（真实事故）：语言服务会把已解析的 lib 与文件 ScriptInfo 缓存在
 * **worker 进程**里；`addExtraLib` 只把新 lib 推过去，**已算过的补全不保证重算**。
 * 于是"后到的正确类型库"进不了已经算出的提示——现场就是状态条写着 18.8.2、
 * 提示却仍是内置 19.x 的 `getGlobalExportByName`。
 *
 * 另一层原因：worker 工厂闭包挂在 `globalThis.MonacoEnvironment` 上（跨 HMR 存活），
 * 开发热更新后新旧模块会各自持有一份缓存，光靠模块变量管不住它。重建 worker 是
 * 唯一确定的"清干净"手段——代价只是一次 worker 冷启（毫秒级，只在类型库真变了时发生）。
 */
function recycleTsWorker(): void {
  for (const label of ["javascript", "typescript"]) {
    const w = workers.get(label);
    if (w) {
      w.terminate();
      workers.delete(label);
    }
  }
}

/**
 * 内置（构建期打进产物）的类型库正文——所有解析路径都失败时的兜底，
 * 也是"打开就有提示"的首次默认值。
 *
 * `gumHasJava` 由正文判定：内置的 gum 19.12.0 已不含 `Java` 命名空间，
 * 所以内置组合需要靠 java-bridge 补 `Java`（垫片会注）。这里是构建期常量，
 * 判一次即可，不必每次运行都扫字符串。
 */
const BUNDLED_GUM_HAS_JAVA = fridaGumDts.includes("declare namespace Java");
export const BUNDLED_FRIDA_TYPES: FridaTypeSet = {
  gum: fridaGumDts,
  javaBridge: fridaJavaBridgeDts,
  gumHasJava: BUNDLED_GUM_HAS_JAVA,
};

/** 已解析到的版本（同版本不重复解析；`Map` 做正文缓存，跨调用复用） */
const typeCache = new Map<string, string>();
let appliedKey: string | null = null;

/**
 * worker（以及它里面的语言服务）**当前是按哪套类型库建起来的**。
 *
 * 与 `appliedKey`（最后注入的那套）分开记：两者不一致就说明"注入了新的、
 * 但 worker 还停在旧的"——那正是补全不跟着版本走的根因。`injectFridaLibs` 里
 * 重建 worker 时把它同步成新指纹；不重建时保持不变（不能盲目同步，
 * 否则就丢掉了"worker 落后了"这个事实）。
 */
let workerLibKey: string | null = null;

/**
 * 解析并注入这个脚本该用的类型库。**来源优先级（为什么是这个顺序）**：
 *
 * 1. **用户指定的项目目录**（设置 → 工具环境 → frida 类型库目录）：这是"与 PyCharm
 *    逐字节同源"的唯一途径——PyCharm 认的就是项目里 `node_modules/@types/frida-gum`，
 *    用户在那里 `npm i` 哪个版本，这里就该是哪个版本。**优先级最高**。
 * 2. **按选定环境的 frida 版本从 CDN 取**：没指定目录时的自动跟随（离线会失败）。
 * 3. **内置**：前两条都不成时的兜底——提示绝不因此消失，只可能滞后一版。
 *
 * 返回给人看的说明（IDE 状态条显示）。同参数重复调用廉价（有缓存与幂等守卫）。
 */
export async function applyFridaTypes(
  fridaVersion: string | null | undefined,
  typesDir: string | null | undefined,
): Promise<string> {
  const dir = typesDir?.trim() ?? "";
  let libs: FridaTypeSet;
  let note: string;

  if (dir) {
    // ① 项目目录（用户明确指定的那份）
    try {
      const { hookApi } = await import("@/api/hook");
      const dto = await hookApi.fridaTypes(dir);
      libs = {
        gum: dto.gum,
        javaBridge: dto.javaBridge ?? null,
        gumHasJava: dto.gumHasJava,
      };
      const gumVer = dto.gumVersion ? `@${dto.gumVersion}` : "";
      const jbVer = dto.javaBridgeVersion ? `@${dto.javaBridgeVersion}` : "";
      note =
        `类型库：@types/frida-gum${gumVer}` +
        (dto.javaBridge ? ` + frida-java-bridge${jbVer}` : "（内含 Java 声明）") +
        `  ← ${dto.gumPath}`;
    } catch (e) {
      // 目录错了要**说出来**（这是用户手动选的，静默回退会让人以为生效了）
      const { resolveFridaTypes } = await import("./fridaTypeVersions");
      const resolved = await resolveFridaTypes(fridaVersion, BUNDLED_FRIDA_TYPES, typeCache);
      libs = {
        gum: resolved.gum.content,
        javaBridge: resolved.javaBridge.content,
        gumHasJava: BUNDLED_GUM_HAS_JAVA,
      };
      note = `指定的类型库目录读不到（${String((e as Error)?.message ?? e)}）：${resolved.note}`;
    }
  } else {
    // ②③ 按 frida 版本解析（内部已含"回退内置"）
    const { resolveFridaTypes } = await import("./fridaTypeVersions");
    const resolved = await resolveFridaTypes(fridaVersion, BUNDLED_FRIDA_TYPES, typeCache);
    const gumHasJava = resolved.gum.content.includes("declare namespace Java");
    libs = {
      gum: resolved.gum.content,
      javaBridge: resolved.javaBridge.content,
      gumHasJava,
    };
    note = resolved.note;
  }

  // 指纹只取**类型库本体**：note 里含路径/版本说明，拿它参与会因文案变化误判成"换了"
  const key = typeSetKey(libs);
  const libsChanged = key !== appliedKey;
  /**
   * ⚠️ 判定"要不要重建 worker"，**不能只看这一拍是否检测到变更**。
   *
   * 事故现场（靠落盘诊断抓到的）：注入的内容明明是正确的 18.8.2
   * （`injected_gum_len=243968 has_findBaseAddress=true`），但 `changed=false` ——
   * 因为**更早那一次已经注入过 18.8.2**，于是此后每一拍都走"没变，什么都不做"，
   * `recycleTsWorker` 再也不触发；而那次注入时 worker 早已按内置 19.x 算过并缓存了
   * 结果 → 表现得像"注入没生效"。
   *
   * 所以判据要改成"**worker 现在还跟这套库一致吗**"：
   * - 库变了 → 必须重建；
   * - 库没变，但 worker 是在**别的库**上建起来的（workerLibKey 对不上）→ 也必须重建；
   * - 两者一致 → 不动（避免每次打开文件都冷启一次 worker）。
   */
  /**
   * 判据是"**worker 现在这个库里建的吗**"，而不是"这一拍有没有变"。
   *
   * `workerLibKey === null` 表示"worker 从没按这套逻辑重建过"——它要么还没建
   * （无所谓，冷启时自然带上新库），要么是按**内置库**建好后我们再没管过它。
   * 两种都以"必须对齐"处理：**只要 worker 已在运行且 key 对不上，就重建**。
   * 唯一不重建的情形是 `workerLibKey === key`（已经一致）。
   */
  const workerInSync = workerLibKey === key;
  // 有 worker 实例 = 它已经在按某套库算了；这种情况下不一致才需要重建
  const workerRunning = workers.size > 0;
  const rebuildNow = !workerInSync && workerRunning;
  if (libsChanged || rebuildNow) {
    injectFridaLibs(libs, rebuildNow);
    appliedKey = key;
  }
  return note;
}

/**
 * 类型库是否已按"选定环境/项目"解析并注入过。
 *
 * 为什么需要这个查询：`setupMonaco()` 会**先注入内置版本**（保证打开就有提示），
 * 而正确的那份要等一个异步 effect 解析完才覆盖。**模型若在这中间建立**，语言服务
 * 就按内置版本算出了结果（那些结果会被缓存下来），此后 extraLib 虽被更新，
 * 已算过的补全不一定会重算——现场表现就是"状态条说用了 18.8.2，但提示还是 19.x 的
 * `getGlobalExportByName`"。所以建模型前要问一句：解析好了吗？
 */
export function fridaTypesApplied(): boolean {
  return appliedKey !== null;
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