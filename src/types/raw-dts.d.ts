/**
 * `?raw` 导入的 frida 类型库资源（Vite 构建期取磁盘字节作为字符串）。
 *
 * 这两个 specifier 经 vite.config 的 `@frida-types/*` 别名指向 node_modules 里的
 * .d.ts 文件——不直接写 `xx.d.ts?raw`，因为 TS 会把以 `.d.ts` 结尾的导入当类型
 * 声明文件拒绝（TS6137）。声明在这里，Monaco IDE 注入类型库时用。
 */
declare module "@frida-types/gum?raw" {
  const content: string;
  export default content;
}
declare module "@frida-types/java-bridge?raw" {
  const content: string;
  export default content;
}
