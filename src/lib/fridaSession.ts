/**
 * frida 会话的启动计划（FT3 路 B）：一次点击要能起**官方 frida REPL**，
 * 并且把"环境够不够"和"这一次连谁、跑哪个脚本"合成一处判断。
 *
 * 为什么合成一处：分两处写就会分叉——一处判 frida-tools、一处拼参数，
 * 中间那个"环境够但参数缺"的空档最后只会表现成"点了没反应"。
 *
 * 环境这一半的口径（D091：**按会话类型分别要求**）：
 * - 终端里的 REPL 需要的是 **frida-tools**（`frida_tools.repl` 就在它里面），
 *   不是"frida 或 frida-tools 任一即通过"：只装绑定包的机器上，那条聚合判断会放行，
 *   然后在屏上收到 Python 的 `No module named frida_tools`。
 * - 启动固定 `<配置好的 python> -m frida_tools.repl`，**不找 PATH 上那个 `frida`**：
 *   brew / pyenv / 系统 Python 混装时 PATH 里的那个很可能属于另一条解释器，
 *   那时"检测说装了、跑起来说没装"这种错查不动；同一条解释器 `-m` 起模块，
 *   检测与执行才是同一个环境。
 *
 * 参数这一半每个形状都对着本机 `python -m frida_tools.repl --help` 抄
 * （2026-09-29，frida 17.17.0）：`-D/-U/-H` 连接、`-f/-F/-p/-n` 目标、`-l` 脚本。
 * 参数数组直接交给 PTY 起进程（不经 shell 分词），所以包名里的空格不用转义，
 * 但拼错一个形状就是起不来，而界面上只会表现为"没反应"。
 */
import type { FridaEnv, PythonEnv } from "@/api/env";
import type { FridaSettings } from "@/features/hook/frida/types";

/** REPL 的模块入口参数（`-m frida_tools.repl`） */
export const FRIDA_REPL_ARGS = ["-m", "frida_tools.repl"];

export type FridaSessionBlock =
  /** Python 没配好或没给出解释器路径 */
  | "no-python"
  /** 那条 Python 里没有 frida-tools */
  | "no-frida-tools"
  /** 没选脚本，或选了但拿不到绝对路径 */
  | "no-script"
  /** spawn 模式必须有包名（官方没有"spawn 前台"这种用法） */
  | "spawn-needs-package"
  /**
   * USB 模式没选定设备。**故意不留 `-U` 兜底**：官方 `-U` 的语义是"随便哪台 USB"，
   * 页面还没选好设备就点脚本时它会静默连到另一台上——而你后面敲的每个 `xxx()` 都打到那台。
   * 设备 shell 那条已经同一个口径。
   */
  | "no-device";

export interface FridaSessionPlan {
  program: string;
  args: string[];
}

export type FridaSessionResult = { ok: true; plan: FridaSessionPlan } | { ok: false; block: FridaSessionBlock };

/**
 * 目标形状 → 官方参数。
 * attach + 空目标是**合法输入**（= 附加设备当前前台，等价 `frida -UF` 的 `-F`），
 * 这条语义必须和 `types.fridaSessionLabel` 一致，否则界面显示"自动前台"而命令是另一回事。
 */
function targetArgs(settings: FridaSettings): string[] | { block: FridaSessionBlock } {
  const target = settings.target.trim();
  if (settings.runMode === "spawn") {
    if (!target) return { block: "spawn-needs-package" };
    return ["-f", target];
  }
  if (!target) return ["-F"];
  if (/^\d+$/.test(target)) return ["-p", target];
  return ["-n", target];
}

/**
 * 连接形状。USB 模式**必须有序列号**（A 区通常已经自动选好），没有就判 `no-device`：
 * `-U` 是"随便哪台 USB"，多设备时会把会话开到她没选的那台上。
 * 远程模式走 adb forward，所以固定 127.0.0.1。
 */
function connectionArgs(settings: FridaSettings): string[] | { block: FridaSessionBlock } {
  if (settings.connMode === "remote") {
    return ["-H", `127.0.0.1:${settings.port.trim() || "27042"}`];
  }
  const serial = settings.deviceSerial?.trim();
  if (!serial) return { block: "no-device" };
  return ["-D", serial];
}

/**
 * 算出这一次 frida 会话的启动计划。
 *
 * `frida` 允许是 null：env 页的剪枝是"Python 没就绪就不去问 frida"，
 * 那种情况下判 `no-python`（先说清前置），而不是假装知道"没装 frida-tools"。
 */
export function planFridaSession(
  python: PythonEnv | null,
  frida: FridaEnv | null,
  settings: FridaSettings,
  scriptPath: string | null | undefined,
): FridaSessionResult {
  if (!python?.ready || !python.path) return { ok: false, block: "no-python" };
  const tools = frida?.fridaToolsVersion;
  if (!tools) return { ok: false, block: "no-frida-tools" };

  const script = scriptPath?.trim();
  // 没脚本一律拒绝：**不许起一个"没加载脚本的 REPL"**——那种会话看着在跑、什么都不钩，最难查
  if (!script) return { ok: false, block: "no-script" };

  const target = targetArgs(settings);
  if ("block" in target) return { ok: false, block: target.block };
  const connection = connectionArgs(settings);
  if ("block" in connection) return { ok: false, block: connection.block };

  return {
    ok: true,
    plan: {
      program: python.path,
      args: [...FRIDA_REPL_ARGS, ...connection, ...target, "-l", script],
    },
  };
}
