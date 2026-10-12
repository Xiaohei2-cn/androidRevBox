/**
 * frida 工作台的前端共享类型与纯函数。
 *
 * 结构化控制台（三轨事件视图）整体退役后，这里只剩**会话设置**这一件事：
 * 启动参数在 `lib/fridaSession` 里组装（官方 REPL），本文件只管它的形状与派生值。
 */

/** A 区会话设置（启动时快照进启动请求） */
export interface FridaSettings {
  deviceSerial: string | null;
  connMode: "usb" | "remote";
  /** 远程模式端口（默认 27042） */
  port: string;
  runMode: "attach" | "spawn";
  /**
   * Spawn：必填，只能是包名。
   * Attach：**留空 = 附加设备当前前台应用**（`frida -UF` 的 `-F`），填了也不看。
   */
  target: string;
}

export const remoteSpec = (s: Pick<FridaSettings, "connMode" | "port">): string | undefined =>
  s.connMode === "remote" ? `127.0.0.1:${s.port.trim() || "27042"}` : undefined;

/** 工作目录 + 名字 → 脚本绝对路径（两端路径分隔符都认；用于编辑器模型键与会话启动） */
export function scriptPath(workdir: string, name: string): string {
  const base = workdir.replace(/[\\/]+$/, "");
  const sep = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  return `${base}${sep}${name}`;
}

/**
 * 新建脚本名的前端把关（与 Rust `is_safe_js_script_name` 同一套规则）：
 * 首字符限字母数字下划线（不以 `.` 或 `-` 开头——后者会被当命令行参数），
 * 其余限字母数字与 `_ . -`，必须以 `.js` 结尾。
 * 这只是为了**在输入框上就能给话**；真正的边界在宿主侧，前端不可信。
 */
export function isSafeJsName(name: string): boolean {
  return /^[A-Za-z0-9_][A-Za-z0-9._-]*\.js$/.test(name.trim());
}