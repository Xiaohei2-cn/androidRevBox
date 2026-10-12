import {
  Archive,
  Binary,
  Braces,
  Hash,
  KeyRound,
  Lock,
  Sigma,
  Type,
  Wrench,
  type LucideIcon,
} from "lucide-react";

/**
 * 分类的图标与色。
 *
 * 为什么写死在这而不是从后端下发：Rust 注册表给的是**结构**（id/参数），
 * 视觉标识属于界面语言（与词典同一层）。图标 + 色块让 9 个分类在折叠态也能扫读，
 * 不用把每一行的字都读一遍。
 *
 * 色用 tailwind 语义色（与 StatusBadge 同一写法），深浅两套主题都留了足够对比。
 * 漏了某个分类时 UI 退回 slate，不会崩。
 */
export interface GroupStyle {
  icon: LucideIcon;
  /** 图标瓦与色点的背景/前景 */
  toneClass: string;
}

const GROUP_STYLES: Record<string, GroupStyle> = {
  encoding: { icon: Binary, toneClass: "bg-sky-500/15 text-sky-600 dark:text-sky-400" },
  digest: { icon: Hash, toneClass: "bg-violet-500/15 text-violet-600 dark:text-violet-400" },
  checksum: { icon: Sigma, toneClass: "bg-cyan-500/15 text-cyan-600 dark:text-cyan-400" },
  kdf: { icon: KeyRound, toneClass: "bg-fuchsia-500/15 text-fuchsia-600 dark:text-fuchsia-400" },
  cipher: { icon: Lock, toneClass: "bg-amber-500/15 text-amber-600 dark:text-amber-400" },
  compression: { icon: Archive, toneClass: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" },
  format: { icon: Braces, toneClass: "bg-indigo-500/15 text-indigo-600 dark:text-indigo-400" },
  text: { icon: Type, toneClass: "bg-rose-500/15 text-rose-600 dark:text-rose-400" },
  misc: { icon: Wrench, toneClass: "bg-slate-500/15 text-slate-600 dark:text-slate-400" },
};

const FALLBACK: GroupStyle = {
  icon: Wrench,
  toneClass: "bg-muted text-muted-foreground",
};

export function groupStyle(category: string): GroupStyle {
  return GROUP_STYLES[category] ?? FALLBACK;
}
