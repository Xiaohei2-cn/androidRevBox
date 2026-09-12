/**
 * 原生文件/目录选择（P8）：Tauri dialog 插件的统一前端出口（api/ 层收口纪律）。
 * 浏览器直开 dev server 时插件不可用——降级返回 null，调用方保持输入框手填可用。
 */
import { open } from "@tauri-apps/plugin-dialog";

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

/** 选择单个目录；取消返回 null */
export async function pickDirectory(title?: string): Promise<string | null> {
  if (!hasTauri()) return null;
  const picked = await open({ multiple: false, directory: true, title });
  return typeof picked === "string" ? picked : null;
}
