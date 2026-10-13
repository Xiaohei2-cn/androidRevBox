/**
 * frida 回调对象字面量的成员补全（补 TypeScript 语言服务的一个"已知不做"）。
 *
 * ## 为什么需要这个文件
 *
 * TS 语言服务对**对象字面量的上下文类型补全**有一个既定行为：**参数类型是 union
 * 时，整个成员补全失效**。实测（TS 5.9.3，与 PyCharm 自带版本相同）：
 *
 * | 参数类型 | `{ ... }` 里补全 |
 * |---|---|
 * | 单一接口 | ✅ 给出 onEnter / onLeave |
 * | union（成员类型一致） | ❌ 不给 |
 * | union（成员类型冲突） | ❌ 不给 |
 *
 * 而 frida 的 `Interceptor.attach` 第二个参数正是 union：
 * `callbacksOrProbe: InvocationListenerCallbacks | InstructionProbeCallback`
 * ——所以 `Interceptor.attach(addr, {` 之后**永远不提示 onEnter / onLeave**。
 *
 * PyCharm 能给是因为它的补全走另一套实现。这里在 Monaco 侧补一个提供者，
 * 只针对"TS 明确不做"的这几处 union 回调位置，**不碰 TS 自己的语义补全**
 * （两者并存由 Monaco 合并）。
 *
 * ## 覆盖范围（刻意保守）
 *
 * 只列**真的有 union 成员问题**的 API；每一条都要有 d.ts 依据，不凭印象加。
 * 加新条目时必须回到 frida-gum 的 d.ts 里核对该 API 的参数类型是不是 union。
 */

import type * as MonacoTypes from "monaco-editor/editor/editor.api";

/** 一处 union 回调：光标在该 API 的配置对象里时，要提示哪些成员 */
interface CallbackSpec {
  /** 该 API 的成员名（用于识别调用位置，如 `attach`） */
  member: string;
  /** 提示的字段：名字 + 类型签名（来自 d.ts，写给人看的悬停说明） */
  fields: Array<{ name: string; detail: string; doc: string }>;
}

/**
 * `Interceptor.attach` / `replace` 等一族的回调字段。
 *
 * 依据（frida-gum d.ts 18.8.2，第 3272-3296 行）：
 * - `ScriptInvocationListenerCallbacks.onEnter?: (this: InvocationContext, args: InvocationArguments) => void`
 * - `ScriptInvocationListenerCallbacks.onLeave?: (this: InvocationContext, retval: InvocationReturnValue) => void`
 * 这里**只给脚本侧**（Script 分支）的形状——用户写的是 JS 脚本，
 * Native 分支（`onEnter?: NativePointer`，CModule 用）对他没用。
 */
const INVOCATION_FIELDS: CallbackSpec["fields"] = [
  {
    name: "onEnter",
    detail: "(this: InvocationContext, args: InvocationArguments) => void",
    doc: "目标函数即将进入时同步调用。`args[i]` 是第 i 个参数的 NativePointer。",
  },
  {
    name: "onLeave",
    detail: "(this: InvocationContext, retval: InvocationReturnValue) => void",
    doc: "目标函数即将返回时同步调用。`retval` 可读可改（`retval.replace(...)`）。",
  },
];

/**
 * 覆盖表：API 成员名 → 该位置该提示的字段。
 *
 * 这些 API 的参数类型在 frida-gum d.ts 里都是 union，因此 TS 不给成员补全。
 */
const SPECS: CallbackSpec[] = [
  { member: "attach", fields: INVOCATION_FIELDS },
  { member: "replaceFast", fields: INVOCATION_FIELDS },
  // `Interceptor.attach` 的 target 写法还有 attachThreadObserver 等，但那些的
  // 参数不在 union 里（TS 自己能补），不需要在此重复。
];

/** 从光标往前扫，判断当前是不是"在某个对象的字面量里" */
interface Ctx {
  /** 识别出的 API 成员名（如 attach） */
  member: string;
  /** 字面量起始的 `{` 相对光标的偏移（-1 = 不在字面量里） */
  braceOffset: number;
}

/**
 * 判断光标是否位于 `apiName(..., { ... | })` 的对象字面量内部。
 *
 * 做法：从光标往前找最近的未配对 `{`；再往前跳过参数，找 `(` 前的成员名。
 * 只做**当前位置的括号配对**，不做完整语法分析——这是补全时的即时判断，
 * 判错也只是"该提示时不提示"，不会给出错误建议。
 */
export function detectCallbackContext(linePrefix: string): Ctx | null {
  // 从光标往前找最近的未配对 `{`
  let depth = 0;
  let braceIdx = -1;
  for (let i = linePrefix.length - 1; i >= 0; i--) {
    const ch = linePrefix[i];
    if (ch === "}") depth++;
    else if (ch === "{") {
      if (depth === 0) {
        braceIdx = i;
        break;
      }
      depth--;
    }
  }
  // 找不到未配对的 `{`：要么不在字面量里，要么字面量已闭合（光标在 `}` 之后）
  if (braceIdx < 0) return null;

  /*
   * 从 `{` 往前找**这次调用的 `(`**：中间允许有别的实参（`attach(addr, {`）。
   * 沿途拒绝会破坏判断的东西（都按"少提示"处理，宁可漏也不要给错建议）：
   * - `(` 配平后命中 ⇒ 认定是实参对象；
   * - 语句/表达式边界：`;` `=` `{` `}` `?` `:` `&&` `||`（三元、赋值、外层块等）
   *   ⇒ 这个 `{` 不是本次调用的实参对象，放弃。
   */
  let depth2 = 0;
  let j = braceIdx - 1;
  let parenIdx = -1;
  const BLOCKERS = [";", "=", "{", "}", "?", ":"];
  for (; j >= 0; j--) {
    const ch = linePrefix[j];
    if (ch === ")") depth2++;
    else if (ch === "(") {
      if (depth2 === 0) {
        parenIdx = j;
        break;
      }
      depth2--;
    } else if (depth2 === 0 && (BLOCKERS.includes(ch) || (ch === "&" || ch === "|"))) {
      return null;
    }
  }
  if (parenIdx < 0) return null;

  // 取 `(` 前的成员名：`Interceptor.attach(`
  let k = parenIdx - 1;
  while (k >= 0 && /[\s]/.test(linePrefix[k])) k--;
  const end = k + 1;
  while (k >= 0 && /[A-Za-z0-9_$]/.test(linePrefix[k])) k--;
  const member = linePrefix.slice(k + 1, end);
  if (!member) return null;
  return { member, braceOffset: braceIdx };
}

/**
 * 注册提供者。**在 setupMonaco 之后调用一次**（幂等由 setupMonaco 的 configured 守卫保证）。
 */
export function registerFridaCallbackCompletions(
  monaco: typeof MonacoTypes,
): MonacoTypes.IDisposable[] {
  const byMember = new Map(SPECS.map((s) => [s.member, s.fields]));

  const provider: MonacoTypes.languages.CompletionItemProvider = {
    triggerCharacters: ["{", ",", "\n", " "],
    provideCompletionItems(model, position) {
      /*
       * ⚠️ 必须取**从文件开头到光标**的文本，不能只取当前行。
       *
       * 真实 bug：最初只取光标所在行 → 用户在第 3 行（`{` 在第 2 行）时，
       * linePrefix 只有 `" "` → 判断不出上下文 → 永远不提示。
       * 用户写 `Interceptor.attach(addr, {` 之后按回车换行再写，这是**最常见的形状**。
       */
      const before = model.getValueInRange({
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: position.lineNumber,
        endColumn: position.column,
      });
      const ctx = detectCallbackContext(before);
      if (!ctx) return { suggestions: [] };
      const fields = byMember.get(ctx.member);
      if (!fields) return { suggestions: [] };

      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };
      return {
        suggestions: fields.map((f) => ({
          label: f.name,
          kind: monaco.languages.CompletionItemKind.Property,
          detail: f.detail,
          documentation: { value: f.doc },
          insertText: `${f.name}: function (${f.name === "onEnter" ? "args" : "retval"}) {\n\t$0\n},`,
          insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
          range,
          // 排在 TS 的兜底项前面（TS 在这个位置只会给全局变量，很噪）
          sortText: `0_${f.name}`,
        })),
      };
    },
  };

  return [
    monaco.languages.registerCompletionItemProvider("javascript", provider),
    monaco.languages.registerCompletionItemProvider("typescript", provider),
  ];
}
