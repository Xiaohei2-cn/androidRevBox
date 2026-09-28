/**
 * frida 交互会话（路 B）的启动前置（FT3 第一步，D091）。
 *
 * 口径是"按会话类型分别要求"，**不是在所有功能上做减法**：
 * - **结构化（轨道）会话**要的是 Python 的 `frida` **绑定包**（我们自己那个 runner 走 `import frida`）；
 * - **终端里的 REPL 会话**要的是 `frida-tools`（`frida_tools.repl` 这个模块在它里面）。
 * 装了 `frida` 没装 `frida-tools` 的机器上，第二条今天会被"任一即通过"放过去，
 * 然后在终端里报一个 Python 的 `No module named frida_tools`——那是把我们的检查口径
 * 漏了，不是用户环境坏了。所以这里单独判，并且只认 `fridaToolsVersion`。
 *
 * 启动方式固定为 `<配置好的 python> -m frida_tools.repl`：
 * 不去找 PATH 里那个叫 `frida` 的命令——它可能属于**另一个** Python（brew/pyenv/系统混装时
 * 十有八九），那样"工具检测说装了、真跑起来说没装"这种错就查不动了。用同一条解释器
 * `-m` 起模块，检测与执行才是同一个环境。
 */
import type { FridaEnv, PythonEnv } from "@/api/env";

/**
 * 官方 REPL 的启动参数。注意它**还缺一个目标**（`-f 包名` / `-n 进程名` / PID）：
 * 预置只负责把"用哪个环境跑哪个模块"填对，目标由用户在参数框里补——
 * 界面必须把这件事说出来，否则点完只会看到 CLI 打一句 usage 就退出。
 */
export const FRIDA_REPL_ARGS = ["-m", "frida_tools.repl"];

export type FridaReplBlock = "no-python" | "no-frida-tools";

export type FridaReplLaunch =
  | { ok: true; program: string; args: string[]; toolsVersion: string }
  | { ok: false; block: FridaReplBlock };

/**
 * 只用**已经探测过**的 env 结果判断，不发请求：探测本身有剪枝（Python 没就绪时
 * 前端根本不会去问 frida），把"没问过"当成"没装"会说谎，所以 `frida` 为 null 时判 no-python。
 */
export function fridaReplLaunch(
  python: PythonEnv | null,
  frida: FridaEnv | null,
): FridaReplLaunch {
  if (!python?.ready || !python.path) return { ok: false, block: "no-python" };
  const tools = frida?.fridaToolsVersion;
  if (!tools) return { ok: false, block: "no-frida-tools" };
  return { ok: true, program: python.path, args: [...FRIDA_REPL_ARGS], toolsVersion: tools };
}
