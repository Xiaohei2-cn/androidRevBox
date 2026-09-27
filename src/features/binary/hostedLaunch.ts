/**
 * 二进制托管的启动偏好与「探测帮助」纯逻辑（UI-6 第一/二层）。
 *
 * 单独立一个文件的原因：这一层里全是"以后要回头核对的判断"——候选顺序、
 * 按事实分类、版本指纹怎么比、参数超限怎么劝。放在组件里就只能靠手点验证，
 * 放在这里每条都能写成断言（见 hostedLaunch.test.ts）。
 */
import type { HostedBinary, HostedProbeResult } from "@/api/device";
// 分词器放在纯 lib 里：从 @/api/task 引它会连带把 @tauri-apps/api/core 拖进
// 这个模块的单测（jsdom 下解析不了），而它本身一行 Tauri 都不需要
import { tokenizeCommand } from "@/lib/commandTokens";

/**
 * 「探测帮助」的候选，按在本机这类 Android/Linux ELF 上**从最常见到最少见**排。
 *
 * 排序理由（都是常见来源的惯例，不是我们发明的）：
 * - `-h`：短标志里最通用的，几乎所有带参数的工具都认；
 * - `--help`：GNU 推荐写法，长标志里最通用；
 * - `-help`：单横杠长格式，Java 系与老式程序常见；
 * - `--usage`：GNU 系只打简短用法；
 * - `help`：Go/Rust 的 CLI 框架常把它当子命令；
 * - `--help-all`：GLib/GIO 框架特色（列全部选项组）；
 * - `-?`：经典 Unix 小众写法（shell 里裸用有历史坑，但 argv 直交不受影响）；
 * - `-H`：少数 Java/编译类工具用大写 H；
 * - `/?`：Windows cmd 的主流，在这台设备上几乎不可能命中，所以排最后。
 *
 * 命中即停。**全部落空时只说"我试过这些都不像"**：有些程序确实把用法写在 man 页
 * 或 stderr 里，也可能它根本就没提供任何 help——不断言"它一定没有帮助输出"。
 */
export const HELP_CANDIDATES: readonly string[] = [
  "-h",
  "--help",
  "-help",
  "--usage",
  "help",
  "--help-all",
  "-?",
  "-H",
  "/?",
];

/** 单候选默认超时（与协议默认一致）；上限也是协议的那个数 */
export const PROBE_TIMEOUT_MS = 4_000;

/**
 * 探测结论的分类——**只用设备报回来的事实**，不用"像不像帮助"这种猜测。
 *
 * 五种形状的差别是有用的信息，尤其是后两种：
 * - `output-exited`：打完就退，最常见的好结果；
 * - `output-hung`：有输出但没退（已经把它杀掉了）；
 * - `silent-exited`：什么都没打就退了 → 换下一个候选；
 * - `silent-hung`：什么都没打也不退 → 最危险，很可能已经进了服务模式（占端口/读写设备）；
 * - `unusable`：连进程都没起来（权限、格式、缺库）。
 */
export type ProbeCategory =
  | "output-exited"
  | "output-hung"
  | "silent-exited"
  | "silent-hung"
  | "unusable";

/** 有没有产出任何输出：按真实字节数判，不看截断后的文本 */
export function probeHasOutput(result: HostedProbeResult): boolean {
  return result.stdout_bytes > 0 || result.stderr_bytes > 0;
}

export function classifyProbe(result: HostedProbeResult): ProbeCategory {
  if (!result.started) return "unusable";
  // timed_out 就是"到点还没退"（设备侧已连进程组一起杀）；
  // 没杀干净时还有 still_running 兜一层，由界面另发警告
  const hung = result.timed_out || result.still_running;
  const noisy = probeHasOutput(result);
  if (hung) return noisy ? "output-hung" : "silent-hung";
  return noisy ? "output-exited" : "silent-exited";
}

/** 这一类值得停下后面的候选吗：只有"确实打东西且自己退了"才算命中 */
export function probeLooksLikeHelp(result: HostedProbeResult): boolean {
  return classifyProbe(result) === "output-exited";
}

/** 分类对应的 i18n 键（界面只按键取文案，不在这儿拼中文） */
export const CATEGORY_KEYS: Record<ProbeCategory, string> = {
  "output-exited": "adb.binary.probe.catOutputExited",
  "output-hung": "adb.binary.probe.catOutputHung",
  "silent-exited": "adb.binary.probe.catSilentExited",
  "silent-hung": "adb.binary.probe.catSilentHung",
  unusable: "adb.binary.probe.catUnusable",
};

/** 一句人话把事实说全：耗时、码/信号、真实输出量 */
export function describeProbeFacts(result: HostedProbeResult): string {
  const bits: string[] = [`${result.elapsed_ms} ms`];
  if (!result.started) return `无法执行 · ${result.detail ?? "原因未知"}`;
  if (result.exit_code !== null && result.exit_code !== undefined) {
    bits.push(`退出码 ${result.exit_code}`);
  }
  if (result.signal !== null && result.signal !== undefined) {
    bits.push(`信号 ${result.signal}`);
  }
  if (result.timed_out) bits.push(result.killed ? "超时已杀" : "超时未杀");
  if (result.still_running) bits.push("仍在运行");
  bits.push(`stdout ${result.stdout_bytes}B / stderr ${result.stderr_bytes}B`);
  if (result.truncated) bits.push("回传已截断");
  return bits.join(" · ");
}

/** 参数与 stdin 的本地预检上限：与协议里那两个数同值（后端与设备还会各拦一道） */
export const MAX_ARGS = 64;
export const MAX_ARG_LEN = 512;
export const MAX_STDIN_BYTES = 64 * 1024;

/** 按 UTF-8 字节数算上限：中文一个字就占 3 字节，按字符数算会双双低估 */
export function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * 把一行输入切成参数数组。
 *
 * 复用 `tokenizeCommand` 的分词（Shell 那套引号规则已被测过），而不是在这里再写一份：
 * 它按"可执行文件 + 参数"切，所以先补一个占位首段再把第一位丢掉——
 * 参数栏里的第一个 token 本身就是参数，不是程序名。
 */
export function splitArgs(line: string): string[] {
  if (!line.trim()) return [];
  const parsed = tokenizeCommand(`__placeholder__ ${line}`);
  return parsed ? parsed.args : [];
}

/**
 * 本地能看出来的问题，提前一句话说明白（最终判据仍在设备/后端）。
 * `root` 决定"含换行的参数"这句话该怎么说：Root 支路按行还原 argv，接不了换行。
 */
export function argsProblem(args: string[], root: boolean): string | null {
  if (args.length > MAX_ARGS) return `参数 ${args.length} 个，超过上限 ${MAX_ARGS}`;
  const tooLong = args.find((a) => utf8Bytes(a) > MAX_ARG_LEN);
  if (tooLong) return `有单个参数超过 ${MAX_ARG_LEN} 字节：${tooLong.slice(0, 24)}…`;
  const multiline = args.find((a) => a.includes("\n") || a.includes("\0"));
  if (multiline) {
    return root
      ? "参数里有换行：Root 支路按行还原参数，接不了这种参数。取消 Root 走 Agent 可以，或去掉这类参数"
      : "参数里有换行（Agent 支路按数组直送，这种参数能带）";
  }
  return null;
}

/** 启动偏好的存储形状：参数、一次性输入、是否保持输入通道、以及当时的版本指纹 */
export interface LaunchPrefs {
  argsText: string;
  stdinText: string;
  interactive: boolean;
  /** 保存那一刻的指纹（见 encodeStamp）；空串 = 没记过 */
  stamp: string;
}

export const emptyLaunchPrefs = (): LaunchPrefs => ({
  argsText: "",
  stdinText: "",
  interactive: false,
  stamp: "",
});

/** 版本指纹：设备上的 mtime + size。不做哈希：这两样在列表现成，且足够指认"文件换过了" */
export interface Stamp {
  size: number;
  mtime: number | null;
}

export function stampOf(binary: Pick<HostedBinary, "size" | "mtimeUnix">): Stamp {
  return { size: binary.size, mtime: binary.mtimeUnix ?? null };
}

export function encodeStamp(stamp: Stamp): string {
  return `${stamp.size}:${stamp.mtime ?? "?"}`;
}

export function decodeStamp(text: string): Stamp | null {
  if (!text) return null;
  const [sizeText, mtimeText] = text.split(":");
  const size = Number(sizeText);
  if (!Number.isFinite(size)) return null;
  if (mtimeText === "?" || mtimeText === undefined || mtimeText === "") {
    return { size, mtime: null };
  }
  const mtime = Number(mtimeText);
  return Number.isFinite(mtime) ? { size, mtime } : { size, mtime: null };
}

/**
 * 比对结果。`unknown` 是有意保留的第三态：
 * Legacy 列表给不出 mtime（只有 Agent 给），这时**不能说"版本没变"**，
 * 也别说"变了"——只能说没核对。
 */
export type StampCompare = "same" | "changed" | "unknown";

export function compareStamp(saved: Stamp | null, current: Stamp): StampCompare {
  if (!saved) return "unknown";
  if (saved.size !== current.size) return "changed";
  if (saved.mtime === null || current.mtime === null) return "unknown";
  return saved.mtime === current.mtime ? "same" : "changed";
}

/** 存储键：设备 + 文件两个维度（与备注同一个口径） */
export const launchKey = (serial: string, name: string) =>
  `adb.binary.launch.${serial}.${name}`;

const clip = (value: string | null, max: number) =>
  value && utf8Bytes(value) > max ? value.slice(0, max) : value ?? "";

export function loadLaunchPrefs(
  serial: string | null,
  name: string,
): LaunchPrefs {
  if (!serial) return emptyLaunchPrefs();
  let raw: LaunchPrefs | null = null;
  try {
    const argsText = localStorage.getItem(`${launchKey(serial, name)}.args`);
    const stdinText = localStorage.getItem(`${launchKey(serial, name)}.stdin`);
    const interactive = localStorage.getItem(`${launchKey(serial, name)}.interactive`);
    const stamp = localStorage.getItem(`${launchKey(serial, name)}.stamp`);
    if (argsText === null && stdinText === null && interactive === null && stamp === null) {
      return emptyLaunchPrefs();
    }
    raw = {
      argsText: clip(argsText, MAX_ARG_LEN * MAX_ARGS),
      stdinText: clip(stdinText, MAX_STDIN_BYTES),
      interactive: interactive === "1",
      stamp: stamp ?? "",
    };
  } catch {
    return emptyLaunchPrefs();
  }
  return raw;
}

/**
 * 保存启动偏好。空值一律**删键**而不是写空串：
 * 留着三个空键，下一次读就分不出"用户清空了"和"这台设备从没存过"。
 */
export function saveLaunchPrefs(serial: string, name: string, prefs: LaunchPrefs): void {
  const set = (suffix: string, value: string | null) => {
    if (value === null || value === "") localStorage.removeItem(`${launchKey(serial, name)}${suffix}`);
    else localStorage.setItem(`${launchKey(serial, name)}${suffix}`, value);
  };
  set(".args", prefs.argsText);
  set(".stdin", prefs.stdinText);
  set(".interactive", prefs.interactive ? "1" : null);
  set(".stamp", prefs.stamp);
}

/** 直接拿分类的 i18n 键：界面与测试都走这一条，避免两边各写一遍查表逻辑 */
export function probeCategoryKey(result: HostedProbeResult): string {
  return CATEGORY_KEYS[classifyProbe(result)];
}

/**
 * 被截断、没显示出来的字节数。
 *
 * 用「真实写出量 − 已回传文本的字节数」而不是直接报上限：界面要能说
 * "后面还有 N 字节没显示"，说成"它输出了 64KB"就是把截断当成了全貌。
 */
export function probeHiddenBytes(result: HostedProbeResult): number {
  const shown = utf8Bytes(result.stdout) + utf8Bytes(result.stderr);
  return Math.max(0, result.stdout_bytes + result.stderr_bytes - shown);
}

/**
 * 探测前的预检：把"点了没反应"变成一句能说清是谁不在的话。
 *
 * 为什么要预检：探测是**写操作**（会在设备上起进程），Agent 不在线时后端一律拒，
 * 不悄悄回退 adb。于是一个断掉的会话在界面上的表现就是"九条候选全部失败"——
 * 用户看到的是"探测不生效"，而真相是"这个功能需要 Agent 在线，而它现在断了"。
 * 那句话必须在动手之前就说，而且要给出可点的出路。
 */
export type ProbePreflight =
  | { kind: "ok" }
  | { kind: "agentOffline"; state: string; detail?: string }
  | { kind: "methodMissing"; agentVersion?: string };

const READY_STATES = new Set(["ready", "degraded"]);

export function probePreflight(status: {
  state: string;
  lastError?: string | null;
  agentVersion?: string | null;
  capabilities?: { method: string; available: boolean }[];
}): ProbePreflight {
  if (!READY_STATES.has(status.state)) {
    return { kind: "agentOffline", state: status.state, detail: status.lastError ?? undefined };
  }
  // 能力表没探完时（空数组）不做判罚：那属于"还不知道"，交给设备侧回答
  const caps = status.capabilities ?? [];
  if (caps.length > 0 && !caps.some((c) => c.method === "hosted.probe" && c.available)) {
    return { kind: "methodMissing", agentVersion: status.agentVersion ?? undefined };
  }
  return { kind: "ok" };
}

/**
 * "桌面命令没注册"的样子：前端比后端新（或反过来）时的典型报错。
 * 不识别成一般的失败，因为它要求的是**重启/重新构建 App**，不是重连设备。
 */
export function looksLikeMissingCommand(message: string): boolean {
  const text = message.toLowerCase();
  return (
    text.includes("not found") ||
    text.includes("does not exist") ||
    text.includes("unknown command") ||
    message.includes("不存在") ||
    message.includes("未注册")
  );
}

/** 同一句话重复九遍不是信息，是噪音：把完全相同的失败原因归并成一条 */
export function repeatedError(errors: (string | undefined)[]): string | null {
  const texts = errors.filter((value): value is string => !!value);
  if (texts.length < 2) return null;
  const first = texts[0];
  return texts.every((text) => text === first) ? first : null;
}

/** 下一级探测最多取这么多个前缀，避免把一张帮助读成一串进程 */
export const MAX_NEXT_LEVEL_PREFIXES = 12;

/**
 * 从**它自己打出来的帮助文本**里取"可以再探一层"的参数前缀。
 *
 * 为什么这不算"扫字面量"：我们不去二进制里捞字符串，只读这次真实输出的 stdout/stderr。
 * 程序愿意打在帮助里的选项，就是它承认的入口；没打在帮助里的我们不猜。
 * 于是多级 help（`xxx -U --help` 还有下一层）不用为每个程序写模板：
 * 第一层探出来的文本自己就是第二层的候选表。
 *
 * 取的是**声明位**的短/长选项（行首缩进后紧跟 `-X` / `--xxx`，可形如 `-h, --help`），
 * 描述里顺便提到的 `-x` 不算；带值的（`--listen=ADDR`、`-p PORT`）也不当前缀
 * ——拿它们去起一次只会因为缺参数而报错，那是一条噪音。
 */
export function extractOptionCandidates(text: string, alreadyTried: string[]): string[] {
  const skip = new Set(alreadyTried);
  const seen = new Set<string>();
  const found: string[] = [];
  const isOption = (token: string) => /^-{1,2}[A-Za-z][\w-]*$/.test(token);
  for (const line of text.split('\n')) {
    // 声明行的形状：缩进 + 选项 [, 选项] + （空格 + 说明）。
    // 说明句里顺带提到的 `--certificate` 不算入口——那是散文，不是它能接的东西。
    // 逗号不能算进 token（`-D,` 不是选项），所以用 [^\s,]+ 取
    const m = /^[ \t]+(-[^\s,]+)(?:[ \t]*,[ \t]*(-[^\s,]+))?/.exec(line);
    if (!m) continue;
    const tokens = [m[1], m[2]].filter((v): v is string => !!v);
    // 带值的选项（`--listen=ADDR`）不当"下一层前缀"：拿它去起一次只会因缺参数报错
    if (tokens.some((token) => token.includes("="))) continue;
    if (!tokens.every(isOption)) continue;
    for (const token of tokens) {
      if (skip.has(token) || seen.has(token)) continue;
      seen.add(token);
      found.push(token);
    }
  }
  return found.slice(0, MAX_NEXT_LEVEL_PREFIXES);
}

/**
 * 判断"这一项到底有没有自己的一层"。
 *
 * 不要为每个前缀盲跑九条候选：绝大多数程序对 `-D --help` 打的就是**同一份**总帮助
 * （GLib 系尤其明显，实测 frida-server 改名的那个二进制就是），跑满九条只是浪费九次进程。
 * 一次的输出与父层比一下就能分三种情况说清楚：
 * - `none`：什么都没打 → 这一项没有下一层（也可能它压根不接受这个前缀）；
 * - `same`：打的是同一份 → 没有独立的一层，别让人误以为"这就是该参数的说明"；
 * - `deeper`：内容不同 → 真有一层，展开给人看，还能继续往下钻。
 */
export type DrillVerdict = "none" | "same" | "deeper";

export function drillVerdict(
  childText: string,
  parentText: string,
  childHasOutput: boolean,
): DrillVerdict {
  if (!childHasOutput) return "none";
  const child = childText.trim();
  const parent = parentText.trim();
  if (parent.length > 0 && child === parent) return "same";
  return "deeper";
}
