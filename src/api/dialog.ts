/**
 * 原生文件/目录选择（P8）：Tauri dialog 插件的统一前端出口（api/ 层收口纪律）。
 * 浏览器直开 dev server 时插件不可用——降级返回 null，调用方保持输入框手填可用。
 */
import { open, save } from "@tauri-apps/plugin-dialog";

/** 有没有原生对话框可用（浏览器直开 dev server 时没有）。
 * 抽出来是因为调用方要分得开「用户取消」和「这里根本弹不了」——把后者说成前者就是骗人。 */
export function hasNativeDialog(): boolean {
  return hasTauri();
}

function hasTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** 选择单个文件；取消返回 null */
export async function pickFile(
  opts: {
    title?: string;
    /** 选择器初始目录 */
    defaultPath?: string;
    filters?: { name: string; extensions: string[] }[];
  } = {},
): Promise<string | null> {
  if (!hasTauri()) return null;
  const picked = await open({ multiple: false, directory: false, ...opts });
  return typeof picked === "string" ? picked : null;
}

/**
 * 多选文件（访达里一次选中多个二进制再打开）；取消返回 null。
 * 与 pickFile 分开是因为 `multiple` 改变返回值形状，混在一个函数里会让调用方猜类型。
 */
export async function pickFiles(
  opts: {
    title?: string;
    defaultPath?: string;
    filters?: { name: string; extensions: string[] }[];
  } = {},
): Promise<string[] | null> {
  if (!hasTauri()) return null;
  const picked = await open({ multiple: true, directory: false, ...opts });
  if (Array.isArray(picked)) return picked;
  return typeof picked === "string" ? [picked] : null;
}

/** 选择单个目录；取消返回 null */
export async function pickDirectory(title?: string): Promise<string | null> {
  if (!hasTauri()) return null;
  const picked = await open({ multiple: false, directory: true, title });
  return typeof picked === "string" ? picked : null;
}

/**
 * 原生「保存」对话框（第七十四轮：控制台「保存日志」）。
 * 取消返回 null —— 取消不是错误，调用方不许把它弹成一句失败。
 * 浏览器直开 dev server 时插件不可用：返回 null，与取消同形，让调用方给一句"只能在桌面版里保存"。
 */
export async function saveFile(opts: {
  title?: string;
  defaultPath?: string;
  filters?: { name: string; extensions: string[] }[];
} = {}): Promise<string | null> {
  if (!hasTauri()) return null;
  const picked = await save(opts);
  return typeof picked === "string" ? picked : null;
}
