/**
 * frida 会话启动计划（FT3 路 B）。
 *
 * 两组断言对应两处真会出错的地方：**环境判错**（只装绑定包的机器被放行，然后屏上报
 * `No module named frida_tools`）与**形状拼错**（表现是"点了没反应"）。
 * 每个形状都对着本机 `python -m frida_tools.repl --help` 抄（2026-09-29，frida 17.17.0）。
 */
import { describe, expect, it } from "vitest";

import type { FridaEnv, PythonEnv } from "@/api/env";
import type { FridaSettings } from "@/features/hook/frida/types";
import { FRIDA_REPL_ARGS, planFridaSession } from "./fridaSession";

const python = (over: Partial<PythonEnv> = {}): PythonEnv => ({
  configured: true,
  ready: true,
  path: "/pyenv/versions/3.13.5/bin/python3",
  version: "Python 3.13.5",
  hint: null,
  ...over,
});

const frida = (over: Partial<FridaEnv> = {}): FridaEnv => ({
  pythonReady: true,
  installed: true,
  fridaVersion: "17.17.0",
  fridaToolsVersion: "13.7.0",
  hint: null,
  ...over,
});

const settings = (over: Partial<FridaSettings> = {}): FridaSettings => ({
  deviceSerial: "18271FDF600FL4",
  connMode: "usb",
  port: "27042",
  runMode: "attach",
  target: "",
  ...over,
});

const SCRIPT = "/Users/me/hooks/hook.js";

describe("环境这一半：只认 frida-tools，且必须用配置的那条解释器", () => {
  it("环境与参数都齐 ⇒ -m frida_tools.repl -D <serial> -F -l <绝对路径>", () => {
    const result = planFridaSession(python(), frida(), settings(), SCRIPT);
    expect(result).toEqual({
      ok: true,
      plan: {
        program: "/pyenv/versions/3.13.5/bin/python3",
        args: [...FRIDA_REPL_ARGS, "-D", "18271FDF600FL4", "-F", "-l", SCRIPT],
      },
    });
  });

  /** D091 的落点：绑定包在、frida-tools 不在 ⇒ 这条会话开不了。 */
  it("只装了 frida 绑定包 ⇒ no-frida-tools（不能被「任一即通过」放过去）", () => {
    expect(
      planFridaSession(python(), frida({ fridaToolsVersion: null, installed: true }), settings(), SCRIPT),
    ).toEqual({ ok: false, block: "no-frida-tools" });
  });

  it("Python 没就绪 / 没给路径 ⇒ no-python；frida 压根没探过也判 no-python", () => {
    expect(planFridaSession(python({ ready: false, path: null }), null, settings(), SCRIPT)).toEqual({
      ok: false,
      block: "no-python",
    });
    expect(planFridaSession(python({ path: null }), frida(), settings(), SCRIPT)).toEqual({
      ok: false,
      block: "no-python",
    });
  });

  /** 没脚本宁可拒绝：**不许起一个"没加载脚本的 REPL"**——看着在跑、什么都不钩，最难查。 */
  it("没选脚本 ⇒ no-script", () => {
    expect(planFridaSession(python(), frida(), settings(), null)).toEqual({ ok: false, block: "no-script" });
    expect(planFridaSession(python(), frida(), settings(), "   ")).toEqual({ ok: false, block: "no-script" });
  });
});

describe("目标与连接形状", () => {
  /** attach + 空目标是合法语义：`-F` = 附加设备当前前台，和 A 区那句「自动前台」同一件事。
   * （标题字段以前在这里断言，现在 tab 一律叫「会话 N」，跑的是什么只在悬停里。） */
  it("attach 空目标 → -F", () => {
    const r = planFridaSession(python(), frida(), settings(), SCRIPT);
    expect(r.ok && r.plan.args).toContain("-F");
  });

  it("attach 填了什么都算前台（-F），只有 spawn 才带包名（-f）", () => {
    // 目标串残留时也不能改形状：界面在 attach 下已经禁用输入框，
    // 万一 settings 里还留着切模式前的包名，会话必须仍然附加前台，而不是去匹配一个进程名。
    for (const target of ["com.example.app", " 12345 "]) {
      const r = planFridaSession(python(), frida(), settings({ target }), SCRIPT);
      expect(r.ok && r.plan.args).toEqual([
        ...FRIDA_REPL_ARGS,
        "-D",
        "18271FDF600FL4",
        "-F",
        "-l",
        SCRIPT,
      ]);
    }
    const spawn = planFridaSession(python(), frida(), settings({ runMode: "spawn", target: "com.x" }), SCRIPT);
    expect(spawn.ok && spawn.plan.args).toEqual([
      ...FRIDA_REPL_ARGS,
      "-D",
      "18271FDF600FL4",
      "-f",
      "com.x",
      "-l",
      SCRIPT,
    ]);
  });

  /** 官方没有"spawn 前台"：缺包名就拒绝，别拼一个 `-f ""` 去撞运气。 */
  it("spawn 不给包名 ⇒ spawn-needs-package", () => {
    expect(planFridaSession(python(), frida(), settings({ runMode: "spawn", target: "" }), SCRIPT)).toEqual({
      ok: false,
      block: "spawn-needs-package",
    });
  });

  /** 多设备时 `-U` 会自己挑"第一台 USB"⇒ 有序列号必须用 `-D` 钉死。 */
  /**
   * USB 模式**必须有序列号**：没选上一台设备就点脚本时，我们拒绝启动，
   * 而不是退回官方那个 `-U`——`-U` 的含义是"随便哪台 USB"，多设备时会把会话开到你没选的那台上，
   * 而你后面敲的每一个 xxx() 都打到那台上。设备 shell 那条已经同样是这个口径。
   */
  it("usb 没选定设备一律拒（不退回 -U）；远程固定 127.0.0.1:<端口>", () => {
    expect(planFridaSession(python(), frida(), settings({ deviceSerial: null }), SCRIPT)).toEqual({
      ok: false,
      block: "no-device",
    });
    expect(planFridaSession(python(), frida(), settings({ deviceSerial: "   " }), SCRIPT)).toEqual({
      ok: false,
      block: "no-device",
    });
    const remote = planFridaSession(
      python(),
      frida(),
      settings({ connMode: "remote", port: "28042", deviceSerial: null }),
      SCRIPT,
    );
    expect(remote.ok && remote.plan.args).toEqual([
      ...FRIDA_REPL_ARGS,
      "-H",
      "127.0.0.1:28042",
      "-F",
      "-l",
      SCRIPT,
    ]);
    const remoteDefault = planFridaSession(python(), frida(), settings({ connMode: "remote", port: " ", deviceSerial: null }), SCRIPT);
    expect(remoteDefault.ok && remoteDefault.plan.args).toContain("127.0.0.1:27042");
  });

  it("脚本路径原样进 -l（含空格也不转义：参数数组是直接 exec 的）", () => {
    const r = planFridaSession(python(), frida(), settings(), "/tmp/my hooks/a b.js");
    const args = r.ok ? r.plan.args : [];
    expect(args[args.length - 2]).toBe("-l");
    expect(args[args.length - 1]).toBe("/tmp/my hooks/a b.js");
  });
});
