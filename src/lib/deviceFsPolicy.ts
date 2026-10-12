/**
 * 设备文件页「增删改」的路径边界（界面侧镜像）。
 *
 * **强制在 Rust**（`src-tauri/src/services/fs_policy.rs`）：这一页默认以 root (su) 执行写操作，
 * root + 任意路径等于让这个工具随手删掉 `/data/data`，所以只放行两棵树 ——
 * `/data/local/tmp`（调试工作目录）与 `/sdcard`（外部存储，含其真身 `/storage/emulated/0`）。
 * 这里带同一份名单，只是为了**提前把按钮禁掉并说清原因**，不让人点了才知道不行。
 *
 * ⚠️ 两处名单必须一起改：改这里而没改 Rust，等于界面放行、后端拒绝（更难懂）；
 * 改 Rust 没改这里，等于按钮能点却吃一个错误。
 */
export const WRITABLE_ROOTS: readonly string[] = [
  "/data/local/tmp",
  "/sdcard",
  "/storage/emulated/0",
];

export type WriteDenial = "not-absolute" | "escapes-root" | "outside";

/** 折叠 `.`/`..`、去掉重复与结尾斜杠；`..` 越出根返回 null（与 Rust 同一套算法） */
export function normalizeDevicePath(path: string): string | null {
  const stack: string[] = [];
  for (const seg of path.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (stack.length === 0) return null;
      stack.pop();
      continue;
    }
    stack.push(seg);
  }
  return `/${stack.join("/")}`;
}

/** 这个路径能不能改：null = 能；否则给出理由（界面按理由配文案） */
export function writeDenial(path: string): WriteDenial | null {
  if (!path.startsWith("/")) return "not-absolute";
  const clean = normalizeDevicePath(path);
  if (clean === null) return "escapes-root";
  for (const root of WRITABLE_ROOTS) {
    if (clean === root || clean.startsWith(`${root}/`)) return null;
  }
  return "outside";
}

export function canWritePath(path: string): boolean {
  return writeDenial(path) === null;
}

/** 界面里那句"允许哪两处"（用规范化的短名单，别把 /storage/emulated/0 也念出来吓人） */
export const writableRootsLabel = () => "/data/local/tmp · /sdcard";
