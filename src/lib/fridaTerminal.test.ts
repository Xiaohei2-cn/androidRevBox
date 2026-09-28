/**
 * frida 终端会话（路 B）的启动前置（FT3 第一步）。
 *
 * 这组断言盯着的是**检查口径**：REPL 只认 frida-tools。
 * 今天桌面侧那句"frida 或 frida-tools 任一即通过"（`hook_service.rs` 的 `frida_ok`）
 * 会放行一台"装了 frida 绑定包、没装 frida-tools"的机器，然后在终端里收到 Python 的
 * `No module named frida_tools`——那是我们的判断漏了，不是用户环境坏了。
 */
import { describe, expect, it } from "vitest";

import type { FridaEnv, PythonEnv } from "@/api/env";
import { FRIDA_REPL_ARGS, fridaReplLaunch } from "./fridaTerminal";

const python = (over: Partial<PythonEnv> = {}): PythonEnv => ({
  configured: true,
  ready: true,
  path: "/Users/tester/.pyenv/versions/3.11.9/bin/python3",
  version: "Python 3.11.9",
  hint: null,
  ...over,
});

const frida = (over: Partial<FridaEnv> = {}): FridaEnv => ({
  pythonReady: true,
  installed: true,
  fridaVersion: "16.5.1",
  fridaToolsVersion: "12.5.3",
  hint: null,
  ...over,
});

describe("frida 终端会话前置（按会话类型分开要求）", () => {
  it("Python 与 frida-tools 都在：起的是同一条解释器上的 -m frida_tools.repl", () => {
    const launch = fridaReplLaunch(python(), frida());
    expect(launch).toEqual({
      ok: true,
      program: "/Users/tester/.pyenv/versions/3.11.9/bin/python3",
      args: ["-m", "frida_tools.repl"],
      toolsVersion: "12.5.3",
    });
  });

  /** 这条就是 D091 的落点：绑定包在、frida-tools 不在 ⇒ 终端会话开不了，且说清缺哪个。 */
  it("只装了 frida 绑定包、没装 frida-tools ⇒ 判开不了（不能被「任一即通过」放过去）", () => {
    const launch = fridaReplLaunch(python(), frida({ fridaToolsVersion: null, installed: true }));
    expect(launch).toEqual({ ok: false, block: "no-frida-tools" });
  });

  it("frida 根本没探过（Python 未就绪时的剪枝）⇒ 先说 Python，不假装知道 frida", () => {
    expect(fridaReplLaunch(python({ ready: false, path: null }), null)).toEqual({
      ok: false,
      block: "no-python",
    });
  });

  /** `ready` 但没有路径是矛盾状态：宁可拒绝启动，也不去猜一个 python 出来。 */
  it("说就绪却没给出解释器路径 ⇒ 判 no-python，不猜", () => {
    expect(fridaReplLaunch(python({ path: null }), frida())).toEqual({ ok: false, block: "no-python" });
  });

  /**
   * 用**配置里那条解释器** `-m` 起模块，而不是找 PATH 里叫 `frida` 的命令：
   * 后者可能属于另一个 Python（brew/pyenv/系统混装），那时"检测说装了、跑起来说没装"查不动。
   */
  it("启动用的 program 就是配置的那条解释器，不是 PATH 上的 frida", () => {
    const launch = fridaReplLaunch(python({ path: "/opt/other/venv/bin/python" }), frida());
    expect(launch.ok && launch.program).toBe("/opt/other/venv/bin/python");
    expect(launch.ok && launch.args[0]).toBe("-m");
  });

  /** 返回值是拷贝：调用方往 args 里补目标时不许把常量本身改掉（别的 tab 也用它）。 */
  it("给的 args 是拷贝，改不动常量", () => {
    const launch = fridaReplLaunch(python(), frida());
    expect(launch.ok).toBe(true);
    if (launch.ok) launch.args.push("-f", "com.example");
    expect(FRIDA_REPL_ARGS).toEqual(["-m", "frida_tools.repl"]);
  });
});
