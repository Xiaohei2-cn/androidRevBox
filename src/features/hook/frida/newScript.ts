/**
 * 「新建脚本」两个入口（B 区脚本条 / 编辑器工具条）共用的一点点规则。
 *
 * 为什么单独成模块：两处都要"默认只落 .js"这条口径，各写一份必然会漂——
 * 用户在工作目录条上敲 `hook_test` 得到 `hook_test.js`，在编辑器里敲同一个名字
 * 却被骂"名字不合法"，同一件事两种脾气。
 */

/**
 * 新脚本的模板正文（头注释点明下一步，比一张白纸友好）。
 *
 * 写成函数而不是模块级常量：模板里那句话是**界面文案**，必须随语言走。
 */
export const newScriptTemplate = (t: (key: string) => string) =>
  `/* ${t("hook.frida.ide.newFileHint")} */
`;

/**
 * 把用户输入归一到 `.js` 文件名（`isSafeJsName` 之前的最后一步）。
 *
 * - 已是 `.js` 结尾：原样返回；
 * - 只有名字、没有扩展名：补 `.js`（`hook_test` → `hook_test.js`）——默认就按 js 建，
 *   不该逼用户记后缀；
 * - **别的扩展名原样交出去**（`hook.txt` → 校验失败并报错）：不静默改名成
 *   `hook.txt.js`，用户敲错了要让他看见，而不是替她猜；
 * - 末尾多余的点先剥掉（`foo.` → `foo.js`），免得造出 `foo..js` 这种怪名字。
 */
export function ensureJsName(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  const base = trimmed.replace(/\.+$/, "");
  if (base.endsWith(".js")) return base;
  if (/\.[^./\\]+$/.test(base)) return base;
  return `${base}.js`;
}
