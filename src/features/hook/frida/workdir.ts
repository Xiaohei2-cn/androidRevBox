/**
 * 脚本工作目录的选择与持久化（B 区「选择目录…」与编辑器空态「选择目录」共用一个实现）。
 *
 * 抽出来的原因：同一件事（弹目录选择器 → 写 config → 回报结果）有两个入口，
 * 两处各写一份迟早分叉（一处写了 config、另一处忘记），而"配置没落盘"这种分叉
 * 用户要重启一次才会发现。
 */
import { configApi } from "@/api/config";
import { pickDirectory } from "@/api/dialog";

/** 配置键（与 Rust KEY_HOOK_WORKDIR 一致，进 ALLOWED_KEYS 白名单） */
export const KEY_HOOK_WORKDIR = "app.hook.workdir";

/**
 * 弹目录选择器并持久化。返回选中的目录；**用户取消返回 null**（取消不是错误）。
 * 写 config 失败会把错误抛出去，由调用方按自己面板的语言说给用户。
 */
export async function pickAndPersistWorkdir(title: string): Promise<string | null> {
  const picked = await pickDirectory(title);
  if (!picked) return null;
  await configApi.set(KEY_HOOK_WORKDIR, picked);
  return picked;
}