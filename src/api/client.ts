import { invoke } from "@tauri-apps/api/core";

/**
 * 前端访问系统能力的唯一出口（P0 约束：禁止在 api/ 之外 invoke）。
 */
export async function invokeCommand<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(cmd, args);
}
