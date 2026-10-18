import { useCallback, useEffect, useRef, useState } from "react";
// 类型只进编译期（import type 被擦除）：编辑器 ref 的形状从 monaco 的模块身份取
import type * as MonacoTypes from "monaco-editor/editor/editor.api";
import {
  AlertTriangle,
  Check,
  FilePlus2,
  FolderOpen,
  Loader2,
  Play,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { hookApi } from "@/api/hook";
import { envApi } from "@/api/env";
import { configApi } from "@/api/config";
import { KEY_FRIDA_TYPES_DIR } from "../workdir";
import { useI18n } from "@/i18n";
import { useSettings } from "@/app/providers";
import { isSafeJsName } from "../types";
import { pickAndPersistWorkdir } from "../workdir";
// **静态导入**是有意的：编辑器壳体的创建必须与守卫**同一拍**完成。
// 之前用 `await import("./monacoSetup")`，await 前后两拍之间存在竞态窗口——
// 两个并发调用（连开两个文件 / StrictMode 双挂载 / HMR 重挂）都能穿过守卫各建一个，
// Monaco 不报错、两个实例垂直叠着："上面一大片空白、下面代码看不全"就是这么来的。
// 体积上不吃亏：本文件已被 React.lazy 包着，整棵 monaco 依赖树仍只进懒加载块。
import { setupMonaco, ensureIdeTheme } from "./monacoSetup";

/**
 * 区域 C · 脚本编辑器（轻量 IDE：语法高亮、折叠、带类型推导的 frida API 补全）。
 *
 * 为什么是 Monaco（而不是 CodeMirror）：智能提示要做成 **PyCharm/frida-agent-example
 * 同源的真类型补全**——`Interceptor.` 出带签名的成员、`Memory.alloc()` 的返回值之后
 * 接着出提示。这要求类语言服务内核，Monaco 自带 TS 语言服务，`addExtraLib` 注入
 * `@types/frida-gum` + `frida-java-bridge` 即可（见 `monacoSetup.ts` 的长注释）。
 * 代价是体积：整个 monaco 走 lazy（本组件只被 React.lazy 引用），不进首屏主块。
 *
 * 文件与模型的关系（编辑器的核心不变量）：
 * - 文件真相在**用户自己的工作目录**（磁盘）：打开 = scriptRead、保存 = scriptWrite；
 * - 每个打开过的文件**各留一份 monaco 模型**（含 undo 栈与未保存改动）——切走再切回，
 *   她敲了一半的东西还在。切文件直接 dispose 旧模型是**丢数据**，不是省内存；
 * - 脏标记按文件记：`dirtyFiles` 集合上报给外层（B 区行上的 ●）。
 */

/** 请求打开某个脚本（由 FridaPage 从 B 区行点击传来） */
export interface OpenRequest {
  name: string;
  nonce: number;
}

type LoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready" }
  | { kind: "error"; message: string };

/** 提示条：一次只显示最近一条（保存成功/失败、删除结果都走这里） */
interface Notice {
  ok: boolean;
  text: string;
}

/** 一个打开过的文件：模型 + 它的"磁盘版本"正文（脏 = 模型正文 ≠ 磁盘版本） */
interface OpenFile {
  model: MonacoTypes.editor.ITextModel;
  /** 最近一次读盘/成功写盘时的正文 */
  saved: string;
}

/**
 * 新建脚本的模板正文（头注释点明下一步，比一张白纸友好）。
 *
 * 写成函数而不是模块级常量：模板里那句话是**界面文案**，必须随语言走。
 * 之前是常量，等于把一句中文钉进代码——`i18n:check` 的硬编码棘轮正是撞在这里。
 */
const newFileTemplate = (t: (key: string) => string) =>
  `/* ${t("hook.frida.ide.newFileHint")} */
`;

export function ScriptIde({
  workdir,
  onWorkdirChange,
  request,
  onRun,
  onChanged,
  onDirtyChange,
}: {
  /** 工作目录（null = 未选择；编辑器给空态） */
  workdir: string | null;
  /** 空态里选了工作目录：目录已落盘 config，这里把新值回报给持有者 */
  onWorkdirChange?: (dir: string) => void;
  /** 外部要求打开某个脚本；nonce 只增，重复点同一个也会生效 */
  request: OpenRequest | null;
  /** 「运行」：把当前（已保存的）文件名交给外层起官方 REPL 会话 */
  onRun: (name: string) => void;
  /** 新建/删除/保存成功后，通知外层刷新 B 区列表 */
  onChanged?: () => void;
  /** 未保存文件的集合变化（B 区行上的 ● 由外层画） */
  onDirtyChange?: (dirty: ReadonlySet<string>) => void;
}) {
  const { t } = useI18n();
  const { effectiveTheme } = useSettings();

  const hostRef = useRef<HTMLDivElement | null>(null);
  /** 编辑器实例与已装载的 monaco 模块 */
  const editorRef = useRef<MonacoTypes.editor.IStandaloneCodeEditor | null>(null);
  const monacoRef = useRef<typeof MonacoTypes | null>(null);
  /** 打开过的文件 → 模型。切文件只换 editor 的当前模型，不 dispose（那是丢数据）。 */
  const filesRef = useRef(new Map<string, OpenFile>());
  /** 各文件的视图状态（光标/滚动/折叠），切回来时原样恢复 */
  const viewStatesRef = useRef(new Map<string, MonacoTypes.editor.ICodeEditorViewState | null>());
  /** 装载序号：懒加载/打开都是异步的，回填前先核对"还是不是我要的那份" */
  const loadSeq = useRef(0);
  const [state, setState] = useState<LoadState>({ kind: "idle" });
  const [current, setCurrent] = useState<string | null>(null);
  /** 未保存的文件集合（按文件记——切文件不丢脏标，也不丢改动） */
  const [dirtyFiles, setDirtyFiles] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 新建行的展开态与名字（null = 收起） */
  const [newName, setNewName] = useState<string | null>(null);
  /** 删除的二次确认（行内出现，不用 window.confirm——桌面 webview 里不可靠） */
  const [confirmDelete, setConfirmDelete] = useState(false);

  /** 空态里选工作目录：与 B 区「选择目录…」同一个实现（弹窗 → 落 config → 回报） */
  const chooseWorkdir = useCallback(async () => {
    setNotice(null);
    try {
      const picked = await pickAndPersistWorkdir(t("hook.frida.pickWorkdirTitle"));
      if (picked) onWorkdirChange?.(picked);
    } catch (e) {
      setNotice({ ok: false, text: String((e as Error)?.message ?? e) });
    }
  }, [onWorkdirChange, t]);

  /** 把某个文件按当前模型内容重算脏状态（模型内容变化的唯一处理口） */
  const recheckDirty = useCallback((name: string) => {
    const file = filesRef.current.get(name);
    if (!file) return;
    const isDirty = file.model.getValue() !== file.saved;
    setDirtyFiles((prev) => {
      if (prev.has(name) === isDirty) return prev;
      const next = new Set(prev);
      if (isDirty) next.add(name);
      else next.delete(name);
      return next;
    });
  }, []);

  // 脏集合上抛给外层（B 区行的 ●）
  const lastReported = useRef<ReadonlySet<string> | null>(null);
  useEffect(() => {
    if (lastReported.current === dirtyFiles) return;
    lastReported.current = dirtyFiles;
    onDirtyChange?.(dirtyFiles);
  }, [dirtyFiles, onDirtyChange]);

  /** 挂载编辑器壳体（单例；切文件只换 model，不重建 editor——重建会闪、会丢布局）。
   *
   * ⚠️ 这里有一个**真实事故**换来的形状：**守卫与创建必须在同一拍完成，中间不许有 await**。
   * 旧写法在 `await import(...)` 前查守卫、await 后直接 create——两拍之间是竞态窗口：
   * 两个并发调用（快速连开两个文件、StrictMode 双挂载、开发热更新重挂）都能穿过去，
   * 在**同一个宿主**上各建一个编辑器，Monaco 不报错，两个实例垂直叠着
   * （实测：第一个空壳 y=1 h=300、第二个真内容 y=301 h=300）——界面上就是
   * "编辑器上面一大片空白、下面代码被容器裁掉看不全"，且先建的那个没人持有引用、
   * 永远不会被 dispose。修法：monacoSetup 改为静态导入（创建同步化），
   * 再留一道宿主自愈（见下）把历史上攒下的孤儿实例清掉。
   */
  const ensureEditor = useCallback(() => {
    if (editorRef.current) return;
    const host = hostRef.current;
    if (!host) return;
    const monaco = setupMonaco();
    monacoRef.current = monaco;
    // 宿主自愈：宿主里若还挂着没人管的编辑器 DOM（历史 HMR 竞态留下的孤儿——
    // 它在 monaco 的实例表里已死或从未被我们持有），先整个清掉再建。
    // 不这么做的现场：新编辑器被追加在孤儿下面，用户在屏上看到的是"上面一片空白"。
    host.replaceChildren();
    editorRef.current = monaco.editor.create(host, {
      theme: ensureIdeTheme(effectiveTheme === "dark"),
      // 视觉口径：行号、折叠、当前行、括号配色——就这几样，别的大件全关
      automaticLayout: true,
      lineNumbers: "on",
      folding: true,
      showFoldingControls: "mouseover",
      lineDecorationsWidth: 4,
      lineNumbersMinChars: 3,
      glyphMargin: false,
      renderLineHighlight: "line",
      bracketPairColorization: { enabled: true },
      matchBrackets: "always",
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      // 20px 行高（13px 字）；写成像素是因为 Monaco 对 <8 的值才按"比例"处理，
      // 直接给精确值省得两套口径混用
      lineHeight: 20,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      padding: { top: 8, bottom: 8 },
      // 补全要"随手就出"：frida 脚本里它顺带就是文档
      quickSuggestions: { other: true, comments: false, strings: false },
      suggestOnTriggerCharacters: true,
      tabCompletion: "on",
      wordBasedSuggestions: "off",
      renderValidationDecorations: "on",
      scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
      fixedOverflowWidgets: true,
    });
  }, [effectiveTheme]);

  // 主题跟随应用明暗（切主题不重建编辑器，只 setTheme）
  useEffect(() => {
    const monaco = monacoRef.current;
    if (!monaco) return;
    monaco.editor.setTheme(ensureIdeTheme(effectiveTheme === "dark"));
  }, [effectiveTheme]);

  /**
   * 类型库跟随**选定环境里的 frida 版本**。
   *
   * 内置的类型库只对某一个 frida 主线正确；用户换版本后内置版给的提示就是错的用法。
   * 这里探一次 frida 版本 → 按版本解析对应类型库 → 覆盖注入。失败/离线就回退内置，
   * 并把结论写到状态条（`ide-types-note`），让人知道现在提示的是哪一版。
   */
  const [typesNote, setTypesNote] = useState<string | null>(null);
  /**
   * 类型库解析的**共享 promise**：打开文件前必须等它落地。
   *
   * 为什么必须等（真实事故）：`setupMonaco()` 会先注**内置版本**保证"打开就有提示"，
   * 正确那份要异步解析完才覆盖。如果模型在这中间建立，语言服务就按内置版本算好了
   * 结果（并被缓存），此后 extraLib 虽更新，已算过的补全不重算——现场就是
   * "状态条写着 18.8.2，提示却仍是 19.x 的 getGlobalExportByName"。
   * 与其在事后补救（重建模型/强制重算都不稳），不如**先备好再建**。
   */
  const typesReadyRef = useRef<Promise<unknown> | null>(null);
  useEffect(() => {
    let alive = true;
    const task = (async () => {
      /**
       * ⚠️ 三个来源**各自容错**，谁也不许拖垮谁。
       *
       * 事故现场：原来写成 `await Promise.all([envApi.frida(), configApi.get(KEY, "")])`，
       * 而 `configApi.get` 对**未注册的配置键**会抛错（Rust 白名单校验）——
       * 于是"读一个可选配置项失败"把整条链路带崩，类型库根本没被换成用户那份，
       * 而面板上一切正常（只有提示内容不对）。这类"可选读取失败"必须降级成本地默认值，
       * 不能中断主线。
       */
      const read = async <T,>(p: Promise<T>, fallback: T): Promise<T> => {
        try {
          return await p;
        } catch {
          return fallback;
        }
      };

      const frida = await read(envApi.frida(), null);
      // 手填的目录（读不到就当没填——旧后端不认识这个键时正是这一条会抛）
      const configuredDir = await read(configApi.get(KEY_FRIDA_TYPES_DIR, ""), "");
      // 没手填目录时**自动发现**：扫常见位置，取最近改动的那个。这样什么都不用配
      // 就能对上 PyCharm 那份（机器上常有多份 frida-agent-example 副本，手填答不对）。
      let dir = configuredDir.trim();
      if (!dir) {
        const found = await read(hookApi.fridaTypesDirs(), [] as Awaited<ReturnType<typeof hookApi.fridaTypesDirs>>);
        dir = found[0]?.projectRoot ?? "";
      }
      try {
        const { applyFridaTypes } = await import("./monacoSetup");
        const note = await applyFridaTypes(frida?.fridaVersion ?? null, dir);
        if (alive) setTypesNote(note);
      } catch (e) {
        // 注入本身失败才落到这里：写清楚（不静默）——用内置版本兜底
        if (alive)
          setTypesNote(
            t("hook.frida.ide.typesInjectFailed", {
              error: String((e as Error)?.message ?? e),
            }),
          );
      }
    })();
    typesReadyRef.current = task;
    return () => {
      alive = false;
    };
  }, []);

  /** 把 editor 切到某个已缓存的模型上（保存/恢复视图状态） */
  const activate = useCallback((name: string) => {
    const editor = editorRef.current;
    const file = filesRef.current.get(name);
    if (!editor || !file) return;
    if (current && current !== name) {
      viewStatesRef.current.set(current, editor.saveViewState());
    }
    editor.setModel(file.model);
    const vs = viewStatesRef.current.get(name);
    if (vs) editor.restoreViewState(vs);
    // 尺寸自愈：编辑器可能是在 Frida 页的"隐藏视图"里创建的（挂载式切页 = 宿主
    // 尺寸为 0），Monaco 会把尺寸钉在最小值且不自动纠回来。每次把模型摆到眼前时
    // 主动量一次，把僵值救回来（对正常路径是一次无副作用的量尺寸）。
    editor.layout();
    editor.focus();
  }, [current]);

  /** 打开文件：有缓存模型就复用（不覆盖未保存改动），没有才读盘 */
  const openFile = useCallback(
    async (name: string) => {
      const seq = ++loadSeq.current;
      setConfirmDelete(false);
      const cached = filesRef.current.get(name);
      if (cached) {
        // 已打开过：直接切过去。**不重读盘**——她敲了一半的东西比磁盘上的新。
        // 但若这份是干净的（没改过），悄悄从盘上刷新一下（可能有外部改动）。
        const clean = cached.model.getValue() === cached.saved;
        if (clean) {
          try {
            const content = await hookApi.scriptRead(name);
            if (seq !== loadSeq.current) return;
            if (content !== cached.saved) {
              cached.saved = content;
              cached.model.setValue(content);
            }
          } catch {
            // 读盘失败但缓存还在：用缓存继续（她在编辑，不该被一次读取失败打断）
          }
        }
        setCurrent(name);
        setState({ kind: "ready" });
        activate(name);
        return;
      }

      setState({ kind: "loading" });
      setNotice(null);
      try {
        /**
         * **先等类型库就绪，再建编辑器与模型**（顺序不能反）。
         *
         * 真实事故：模型若在"内置类型库还在生效"的窗口里建立，语言服务就按内置
         * （19.x）算出了 Module 的补全并缓存；此后 extraLib 虽被换成 18.8.2，
         * 已算过的结果不重算——现场就是"状态条写着 18.8.2，提示却仍是
         * getGlobalExportByName"。等一下就彻底没有这个窗口。
         * 等的是**共享 promise**（见 typesReadyRef），不会重复解析；它自带容错，
         * 最坏情况等价于"用内置版本"（老行为），不会把 IDE 卡住。
         */
        await typesReadyRef.current;
        // 同步建壳：守卫与创建必须同一拍完成，中间不许有 await（无竞态窗口）
        ensureEditor();
        const content = await hookApi.scriptRead(name);
        // 期间又切了别的文件：这份结果作废
        if (seq !== loadSeq.current) return;
        const monaco = monacoRef.current;
        if (!monaco || !editorRef.current) return;

        // 二次核对（并发打开同一个文件时可能已经有缓存了）
        const again = filesRef.current.get(name);
        if (again) {
          setCurrent(name);
          setState({ kind: "ready" });
          activate(name);
          return;
        }

        const uri = monaco.Uri.parse(`inmemory://frida/${name}`);
        const model = monaco.editor.createModel(content, "javascript", uri);
        model.setEOL(monaco.editor.EndOfLineSequence.LF);
        filesRef.current.set(name, { model, saved: content });
        // 脏标记：内容一变就与"磁盘版本"比对。监听挂在模型上，模型 dispose 时一起走。
        model.onDidChangeContent(() => recheckDirty(name));

        setCurrent(name);
        setState({ kind: "ready" });
        activate(name);
      } catch (e) {
        if (seq !== loadSeq.current) return;
        setCurrent(null);
        setState({ kind: "error", message: String((e as Error)?.message ?? e) });
      }
    },
    [activate, ensureEditor, recheckDirty],
  );

  // 外部打开请求（nonce 变化即重新打开；重复点同一个也生效）
  const lastNonce = useRef<number | null>(null);
  useEffect(() => {
    if (!request || !workdir) return;
    if (request.nonce === lastNonce.current) return;
    lastNonce.current = request.nonce;
    void openFile(request.name);
  }, [request, workdir, openFile]);

  /** 放掉某一个文件的模型（删除/目录清空时用） */
  const dropFile = useCallback((name: string) => {
    const file = filesRef.current.get(name);
    if (!file) return;
    file.model.dispose();
    filesRef.current.delete(name);
    viewStatesRef.current.delete(name);
    setDirtyFiles((prev) => {
      if (!prev.has(name)) return prev;
      const next = new Set(prev);
      next.delete(name);
      return next;
    });
  }, []);

  // 工作目录被清掉：放掉全部模型
  useEffect(() => {
    if (workdir) return;
    loadSeq.current += 1;
    for (const name of [...filesRef.current.keys()]) dropFile(name);
    setCurrent(null);
    setState({ kind: "idle" });
  }, [workdir, dropFile]);

  // 组件卸载：编辑器实例与全部模型一起放掉。
  // 两个 Map 在 effect 建立时捕获一次——卸载时 ref.current 可能已被改写，
  // 捕获的那份才是"这个挂载实例自己建的表"（延时器同理）。
  // 共享的创建 promise 也要清掉：StrictMode 双挂载下第二次挂载必须能重新创建
  // （否则会拿着上一次已 resolve 的 promise 直接跳过创建，编辑器永远不出现）。
  useEffect(() => {
    const files = filesRef.current;
    const viewStates = viewStatesRef.current;
    return () => {
      editorRef.current?.dispose();
      editorRef.current = null;
      for (const file of files.values()) file.model.dispose();
      files.clear();
      viewStates.clear();
    };
  }, []);

  /** 写盘。返回是否成功（新建流程要据此决定要不要继续打开） */
  const write = useCallback(
    async (name: string, content: string, createOnly: boolean): Promise<boolean> => {
      setBusy(true);
      setNotice(null);
      try {
        const bytes = await hookApi.scriptWrite(name, content, createOnly);
        setNotice({ ok: true, text: t("hook.frida.ide.savedBytes", { bytes }) });
        onChanged?.();
        return true;
      } catch (e) {
        setNotice({ ok: false, text: String((e as Error)?.message ?? e) });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [onChanged, t],
  );

  /** 保存当前文件 */
  const saveCurrent = useCallback(async () => {
    const file = current ? filesRef.current.get(current) : undefined;
    if (!current || !file) return;
    const content = file.model.getValue();
    if (await write(current, content, false)) {
      file.saved = content;
      recheckDirty(current);
    }
  }, [current, write, recheckDirty]);

  /** 新建：从模板建一个空脚本，成功后立刻打开 */
  const createFile = useCallback(async () => {
    const name = (newName ?? "").trim();
    if (!isSafeJsName(name)) {
      setNotice({ ok: false, text: t("hook.frida.ide.badName") });
      return;
    }
    if (await write(name, newFileTemplate(t), true)) {
      setNewName(null);
      await openFile(name);
    }
  }, [newName, write, openFile, t]);

  /** 删除当前文件（行内二次确认之后才会走到这里） */
  const deleteCurrent = useCallback(async () => {
    if (!current) return;
    const name = current;
    setBusy(true);
    setNotice(null);
    try {
      await hookApi.scriptDelete(name);
      loadSeq.current += 1;
      dropFile(name);
      setCurrent(null);
      setState({ kind: "idle" });
      setConfirmDelete(false);
      setNotice({ ok: true, text: t("hook.frida.ide.deleted", { name }) });
      onChanged?.();
    } catch (e) {
      setNotice({ ok: false, text: String((e as Error)?.message ?? e) });
    } finally {
      setBusy(false);
    }
  }, [current, dropFile, onChanged, t]);

  /** Ctrl/Cmd+S 保存（编辑器聚焦时它也是保存；不聚焦时同样是） */
  const saveRef = useRef(saveCurrent);
  saveRef.current = saveCurrent;
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const dirty = current ? dirtyFiles.has(current) : false;

  return (
    <section className="flex h-full min-h-0 flex-col gap-2 rounded-xl border border-border/70 bg-card shadow-card p-2.5 text-xs">
      {/* 工具条：当前文件（含脏标）、保存、运行、新建、删除 */}
      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
        <span
          className="min-w-0 flex-1 truncate font-mono text-11px"
          title={current ? `${workdir ?? ""}/${current}` : undefined}
          data-testid="ide-current-file"
        >
          {current ?? <span className="text-muted-foreground">{t("hook.frida.ide.noFile")}</span>}
          {dirty && (
            <span className="pl-1 text-amber-500" data-testid="ide-dirty">
              ●
            </span>
          )}
        </span>

        <Button
          size="sm"
          variant="outline"
          className="h-6 shrink-0 gap-1 px-2 text-11px"
          disabled={!current || busy}
          onClick={() => void saveCurrent()}
          data-testid="ide-save"
          title={t("hook.frida.ide.saveHint")}
        >
          {busy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Save className="h-3 w-3" />}
          {t("hook.frida.ide.save")}
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="h-6 shrink-0 gap-1 px-2 text-11px"
          disabled={!current || dirty || busy}
          onClick={() => current && onRun(current)}
          data-testid="ide-run"
          title={dirty ? t("hook.frida.ide.saveFirst") : t("hook.frida.ide.runHint")}
        >
          <Play className="h-3 w-3" />
          {t("hook.frida.ide.run")}
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="h-6 shrink-0 gap-1 px-2 text-11px"
          disabled={!workdir || busy}
          onClick={() => setNewName("")}
          data-testid="ide-new"
        >
          <FilePlus2 className="h-3 w-3" />
          {t("hook.frida.ide.new")}
        </Button>

        {confirmDelete ? (
          <>
            <span className="shrink-0 text-11px text-muted-foreground">
              {t("hook.frida.ide.deleteConfirm", { name: current ?? "" })}
            </span>
            <Button
              size="sm"
              variant="outline"
              className="h-6 shrink-0 gap-1 px-2 text-11px text-destructive"
              disabled={busy}
              onClick={() => void deleteCurrent()}
              data-testid="ide-delete-confirm"
            >
              <Trash2 className="h-3 w-3" />
              {t("hook.frida.ide.delete")}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="h-6 shrink-0 px-2 text-11px"
              onClick={() => setConfirmDelete(false)}
            >
              {t("common.cancel")}
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            variant="outline"
            className="h-6 shrink-0 gap-1 px-2 text-11px"
            disabled={!current || busy}
            onClick={() => setConfirmDelete(true)}
            data-testid="ide-delete"
            title={t("hook.frida.ide.deleteHint")}
          >
            <Trash2 className="h-3 w-3" />
          </Button>
        )}
      </div>

      {/* 新建行：名字输入 + 确定/取消（就在工具条下一行，不开对话框） */}
      {newName !== null && (
        <div className="flex shrink-0 items-center gap-1.5" data-testid="ide-new-row">
          <input
            autoFocus
            aria-label={t("hook.frida.ide.newName")}
            className="h-6 min-w-40 flex-1 rounded-md border border-input bg-background/70 px-1.5 font-mono text-11px focus-visible:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            placeholder="hook_xxx.js"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void createFile();
              if (e.key === "Escape") setNewName(null);
            }}
          />
          <Button
            size="sm"
            className="h-6 shrink-0 gap-1 px-2 text-11px"
            disabled={busy}
            onClick={() => void createFile()}
            data-testid="ide-new-create"
          >
            {t("hook.frida.ide.create")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-6 shrink-0 px-2 text-11px"
            onClick={() => setNewName(null)}
            aria-label={t("common.cancel")}
          >
            <X className="h-3 w-3" />
          </Button>
        </div>
      )}

      {/*
        宿主**常显**：欢迎面板是盖在上面的绝对定位层。绝不能把宿主切 display:none——
        编辑器一旦在"不可见宿主"里创建，Monaco 量到 0 尺寸并钉在最小值 5×5 不再自纠
        （实测现场：编辑器只有 5px 高、代码一行都看不见）。可见性交给上面的覆盖层表达。
      */}
      <div className="relative min-h-0 flex-1 overflow-hidden rounded-lg border border-border/60">
        <div ref={hostRef} className="absolute inset-0" data-testid="ide-editor-host" />

        {state.kind === "idle" && (
          <div
            className="absolute inset-0 overflow-y-auto bg-card px-5 py-4"
            data-testid="ide-welcome"
          >
            {/*
              欢迎面板**顶部对齐**（用户反馈：原来那句"居中一句话"把上半屏留成一大片空）。
              这里把空态当成一块正经内容来排：图标 + 标题 + 说明 + 直接可点的动作，
              底下给三条上手提示——空着的编辑器区域应当是"下一步怎么做"的说明书，
              不是一句飘在中间的话。
            */}
            <div className="flex flex-col items-start gap-3">
              <span className="flex h-10 w-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
                {workdir ? <FilePlus2 className="h-5 w-5" /> : <FolderOpen className="h-5 w-5" />}
              </span>
              <div className="flex flex-col gap-1">
                <p className="text-sm font-medium">
                  {workdir ? t("hook.frida.ide.welcomeTitle") : t("hook.frida.ide.noWorkdir")}
                </p>
                <p className="max-w-xl text-11px leading-relaxed text-muted-foreground">
                  {workdir ? t("hook.frida.ide.welcomeBody") : t("hook.frida.ide.noWorkdirBody")}
                </p>
              </div>

              <div className="flex flex-wrap items-center gap-1.5">
                {workdir ? (
                  <>
                    <Button
                      size="sm"
                      className="h-7 gap-1 px-2.5 text-11px"
                      onClick={() => setNewName("")}
                      data-testid="ide-welcome-new"
                    >
                      <FilePlus2 className="h-3 w-3" />
                      {t("hook.frida.ide.new")}
                    </Button>
                    <span className="path-selectable min-w-0 truncate font-mono text-11px text-muted-foreground" title={workdir}>
                      {t("hook.frida.ide.workdirLabel")}: {workdir}
                    </span>
                  </>
                ) : (
                  <Button
                    size="sm"
                    className="h-7 gap-1 px-2.5 text-11px"
                    onClick={() => void chooseWorkdir()}
                    data-testid="ide-welcome-pick"
                  >
                    <FolderOpen className="h-3 w-3" />
                    {t("hook.frida.ide.pickDirCta")}
                  </Button>
                )}
              </div>

              {/* 三条上手提示：把"这块面板能干什么"说在明处（每一条都对着一个真按钮） */}
              <ul className="mt-1 flex flex-col gap-1.5 text-11px leading-relaxed text-muted-foreground">
                <li className="flex items-start gap-1.5">
                  <span className="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-muted-foreground/60" />
                  {t("hook.frida.ide.tipOpen")}
                </li>
                <li className="flex items-start gap-1.5">
                  <span className="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-muted-foreground/60" />
                  {t("hook.frida.ide.tipSaveRun")}
                </li>
                <li className="flex items-start gap-1.5">
                  <span className="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-muted-foreground/60" />
                  {t("hook.frida.ide.tipCompletion")}
                </li>
              </ul>
            </div>
          </div>
        )}

        {state.kind === "loading" && (
          <div className="absolute inset-0 flex items-center justify-center gap-2 bg-card text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("hook.frida.ide.loading")}
          </div>
        )}

        {state.kind === "error" && (
          <div className="absolute inset-0 flex items-center justify-center bg-card p-4">
            <p className="max-w-md break-all rounded border border-destructive/50 bg-destructive/10 px-2 py-1 text-center text-11px text-destructive">
              <AlertTriangle className="mr-1 inline h-3 w-3" />
              {state.message}
            </p>
          </div>
        )}
      </div>

      {typesNote && (
        <p
          className="shrink-0 truncate text-10px text-muted-foreground"
          title={typesNote}
          data-testid="ide-types-note"
        >
          {typesNote}
        </p>
      )}

      {notice && (
        <p
          className={`shrink-0 break-all rounded px-1.5 py-1 text-11px ${
            notice.ok
              ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
              : "bg-destructive/10 text-destructive"
          }`}
          data-testid="ide-notice"
        >
          {notice.ok ? <Check className="mr-1 inline h-3 w-3" /> : null}
          {notice.text}
        </p>
      )}
    </section>
  );
}