import { beforeEach, describe, expect, it } from "vitest";
import type { HostedProbeResult } from "@/api/device";
import {
  HELP_CANDIDATES,
  MAX_ARG_LEN,
  MAX_ARGS,
  argsProblem,
  classifyProbe,
  judgeProbe,
  compareStamp,
  decodeStamp,
  emptyLaunchPrefs,
  encodeStamp,
  loadLaunchPrefs,
  looksLikeMissingCommand,
  probeHasOutput,
  probePreflight,
  repeatedError,
  probeLooksLikeHelp,
  saveLaunchPrefs,
  splitArgs,
  stampOf,
  utf8Bytes,
} from "./hostedLaunch";

/** 造一条探测回执：只填关心的几项，其余按"没发生"给默认 */
function probe(part: Partial<HostedProbeResult> = {}): HostedProbeResult {
  return {
    args: ["-h"],
    started: true,
    pid: 1234,
    exit_code: 0,
    signal: null,
    timed_out: false,
    killed: false,
    still_running: false,
    stdout: "Usage: toybox [-h]",
    stderr: "",
    stdout_bytes: 20,
    stderr_bytes: 0,
    truncated: false,
    elapsed_ms: 12,
    detail: null,
    ...part,
  };
}

describe("HELP_CANDIDATES 的排序", () => {
  it("从最常见的短帮助开始，Windows 那套排最后", () => {
    expect(HELP_CANDIDATES.slice(0, 2)).toEqual(["-h", "--help"]);
    expect(HELP_CANDIDATES[HELP_CANDIDATES.length - 1]).toBe("/?");
    expect(new Set(HELP_CANDIDATES).size).toBe(HELP_CANDIDATES.length);
  });

  it("用户点过名的几种写法都在表里", () => {
    for (const candidate of ["-h", "--help", "help", "-?", "/?", "--help-all", "-help", "--usage", "-H"]) {
      expect(HELP_CANDIDATES).toContain(candidate);
    }
  });
});

describe("classifyProbe 只按设备事实分类", () => {
  it("五种形状各归一位", () => {
    expect(classifyProbe(probe())).toBe("output-exited");
    expect(classifyProbe(probe({ timed_out: true, killed: true }))).toBe("output-hung");
    expect(classifyProbe(probe({ stdout: "", stdout_bytes: 0 }))).toBe("silent-exited");
    expect(
      classifyProbe(probe({ stdout: "", stdout_bytes: 0, timed_out: true, killed: true })),
    ).toBe("silent-hung");
    expect(classifyProbe(probe({ started: false, detail: "not_executable: 缺少执行位" }))).toBe(
      "unusable",
    );
  });

  it("只有 stderr 有输出也算「有输出」：报错也是一种反应", () => {
    expect(
      classifyProbe(probe({ stdout: "", stdout_bytes: 0, stderr: "unknown option", stderr_bytes: 14 })),
    ).toBe("output-exited");
  });

  it("没杀干净（仍在运行）必须归到「没退出」那一侧，不能算成功", () => {
    expect(classifyProbe(probe({ still_running: true }))).toBe("output-hung");
  });

  it("命中判定只认「打完就退」", () => {
    expect(probeLooksLikeHelp(probe())).toBe(true);
    expect(probeLooksLikeHelp(probe({ timed_out: true }))).toBe(false);
    expect(probeLooksLikeHelp(probe({ stdout: "", stdout_bytes: 0 }))).toBe(false);
  });

  it("真实输出量按字节看，不看截断后的文本", () => {
    expect(probeHasOutput(probe({ stdout: "", stdout_bytes: 4_096, truncated: true }))).toBe(true);
  });

  it("结论只回答有/没有：有输出的那条就是「有」", () => {
    const hit = judgeProbe([
      { candidate: "-h", result: probe({ stdout: "", stdout_bytes: 0 }) },
      { candidate: "--help", result: probe({ stdout: "Usage: demo\n", stdout_bytes: 12 }) },
    ]);
    expect(hit.kind).toBe("help");
    if (hit.kind === "help") {
      expect(hit.text).toContain("Usage");
      // 数字要能继续说清"完不完整、留没留东西"，所以原始回执跟着结论一起交出去
      expect(hit.result.stdout_bytes).toBe(12);
    }
  });

  it("只有 stderr 有内容也算「有」，并且就展示那份", () => {
    const hit = judgeProbe([
      {
        candidate: "--help",
        result: probe({ stdout: "", stdout_bytes: 0, stderr: "用法：demo [-h]", stderr_bytes: 20 }),
      },
    ]);
    expect(hit.kind).toBe("help");
    if (hit.kind === "help") expect(hit.text).toContain("用法：demo");
  });

  it("打完东西没退（被判超时杀掉）仍然是「有」——内容比姿态重要", () => {
    const hit = judgeProbe([
      { candidate: "--help", result: probe({ timed_out: true, killed: true, still_running: true }) },
    ]);
    expect(hit.kind).toBe("help");
    if (hit.kind === "help") expect(hit.stillRunning).toBe(true);
  });

  it("一条内容都没有就是「没有」，并把能查到的原因带出来", () => {
    expect(judgeProbe([{ candidate: "-h", result: probe({ stdout: "", stdout_bytes: 0 }) }])).toEqual({
      kind: "none",
      detail: undefined,
    });
    const refused = judgeProbe([
      {
        candidate: "-h",
        result: probe({ started: false, stdout: "", stdout_bytes: 0, detail: "not_an_elf: 不是 ELF" }),
      },
    ]);
    expect(refused.kind).toBe("none");
    if (refused.kind === "none") expect(refused.detail).toContain("not_an_elf");
    const failed = judgeProbe([{ candidate: "-h", error: "Agent 不在线" }]);
    expect(failed.kind === "none" && failed.detail).toBe("Agent 不在线");
    expect(judgeProbe([])).toEqual({ kind: "none", detail: undefined });
  });
});

describe("splitArgs 的引号规则", () => {
  it("普通空格切开", () => {
    expect(splitArgs("-l 127.0.0.1:27042")).toEqual(["-l", "127.0.0.1:27042"]);
  });

  it("引号包住的空格算一个参数", () => {
    expect(splitArgs('--title "a b" -x')).toEqual(["--title", "a b", "-x"]);
    expect(splitArgs("--title 'a b'")).toEqual(["--title", "a b"]);
  });

  it("空输入与全空白都是空数组", () => {
    expect(splitArgs("")).toEqual([]);
    expect(splitArgs("   \t ")).toEqual([]);
  });

  it("分号不参与切分（它就是个普通字符）", () => {
    expect(splitArgs("a;b -c")).toEqual(["a;b", "-c"]);
  });
});

/**
 * 参数栏是**透传**：切分不改写任何字符。
 *
 * 曾经在这里做过"排版字符 → ASCII"的归一化，是错的方向——它会把输入法把两个 `-`
 * 合成一个 `—` 这件事替用户"修好"，用户看到的反而是"打第二个横杠时横杠被吞了"，
 * 而真因被一句"已替换"盖住。字符问题在输入框解决（关智能替换），不在下游猜。
 */
describe("splitArgs 原样透传", () => {
  it("排版字符不做任何替换，原样进 argv", () => {
    const input = "-p 11881 \u2014\u2014bak 0x7eee8c6000:r -\u2014bro-len 4 \u2014\u2014stack";
    expect(splitArgs(input)).toEqual([
      "-p",
      "11881",
      "\u2014\u2014bak",
      "0x7eee8c6000:r",
      "-\u2014bro-len",
      "4",
      "\u2014\u2014stack",
    ]);
  });

  it("引号内的空格仍然是一个参数", () => {
    expect(splitArgs("-p 13425 --b")).toEqual(["-p", "13425", "--b"]);
    expect(splitArgs('--title "a b" -x')).toEqual(["--title", "a b", "-x"]);
  });

  it("两个连续横杠不会被合并", () => {
    expect(splitArgs("-p 13425 --b")).toEqual(["-p", "13425", "--b"]);
    expect(splitArgs("--").join(" ")).toBe("--");
  });
});

describe("argsProblem 的预检", () => {
  it("个数与单长度上限与协议同值", () => {
    expect(argsProblem(Array.from({ length: MAX_ARGS }, () => "x"), false)).toBeNull();
    expect(argsProblem(Array.from({ length: MAX_ARGS + 1 }, () => "x"), false)).toContain("超过上限");
    const long = "啊".repeat(200); // 600 字节 > 512
    expect(utf8Bytes(long)).toBeGreaterThan(MAX_ARG_LEN);
    expect(argsProblem([long], false)).toContain("字节");
  });

  it("含换行的参数：Root 支路说「接不了并给出换路办法」，Agent 支路只说明白", () => {
    expect(argsProblem(["a\nb"], true)).toContain("取消 Root");
    expect(argsProblem(["a\nb"], true)).toContain("按行还原");
    const agent = argsProblem(["a\nb"], false);
    expect(agent).not.toContain("取消 Root");
  });
});

describe("版本指纹（mtime + size）", () => {
  beforeEach(() => localStorage.clear());

  it("编解码往返回到同一个数", () => {
    const stamp = { size: 2048, mtime: 1_760_000_000 };
    expect(decodeStamp(encodeStamp(stamp))).toEqual(stamp);
    expect(decodeStamp("")).toBeNull();
    expect(decodeStamp("abc:x")).toBeNull();
  });

  it("Legacy 给不出 mtime 时是 ?，不是猜一个数", () => {
    expect(encodeStamp({ size: 10, mtime: null })).toBe("10:?");
    expect(decodeStamp("10:?")).toEqual({ size: 10, mtime: null });
    expect(stampOf({ size: 10, mtimeUnix: undefined })).toEqual({ size: 10, mtime: null });
  });

  it("size 变了就是变了；任一边没有 mtime 只能说「没核对」", () => {
    const saved = { size: 100, mtime: 5 };
    expect(compareStamp(saved, { size: 101, mtime: 5 })).toBe("changed");
    expect(compareStamp(saved, { size: 100, mtime: 6 })).toBe("changed");
    expect(compareStamp(saved, { size: 100, mtime: 5 })).toBe("same");
    expect(compareStamp(saved, { size: 100, mtime: null })).toBe("unknown");
    expect(compareStamp({ size: 100, mtime: null }, { size: 100, mtime: 5 })).toBe("unknown");
    expect(compareStamp(null, { size: 100, mtime: 5 })).toBe("unknown");
  });

  it("参数按设备+文件存，且空值不占键", () => {
    saveLaunchPrefs("SERIAL1", "toybox", { argsText: "-l 27042", stamp: "100:5" });
    expect(localStorage.getItem("adb.binary.launch.SERIAL1.toybox.args")).toBe("-l 27042");
    expect(loadLaunchPrefs("SERIAL1", "toybox")).toEqual({ argsText: "-l 27042", stamp: "100:5" });
    // 换一台设备不该串味
    expect(loadLaunchPrefs("OTHER", "toybox")).toEqual(emptyLaunchPrefs());
    // 用户清空后读回来仍是空，且不留下空键
    saveLaunchPrefs("SERIAL1", "toybox", emptyLaunchPrefs());
    expect(localStorage.getItem("adb.binary.launch.SERIAL1.toybox.args")).toBeNull();
    expect(loadLaunchPrefs("SERIAL1", "toybox")).toEqual(emptyLaunchPrefs());
  });
});

describe("探测前的预检（把「点了没反应」说成一句人话）", () => {
  it("Agent 会话没就绪就不去起进程", () => {
    const verdict = probePreflight({ state: "disconnected", lastError: "adb forward 失败" });
    expect(verdict.kind).toBe("agentOffline");
    if (verdict.kind === "agentOffline") {
      expect(verdict.state).toBe("disconnected");
      expect(verdict.detail).toContain("forward");
    }
  });

  it("Agent 在线但没宣告 hosted.probe 判成「版本旧」", () => {
    expect(
      probePreflight({
        state: "ready",
        agentVersion: "0.2.1",
        capabilities: [{ method: "hosted.start", available: true }],
      }).kind,
    ).toBe("methodMissing");
    expect(
      probePreflight({
        state: "degraded",
        agentVersion: "0.2.2",
        capabilities: [
          { method: "hosted.start", available: true },
          { method: "hosted.probe", available: true },
        ],
      }).kind,
    ).toBe("ok");
  });

  it("能力表还没探完（空）时不下判语：那属于不知道，不是不可用", () => {
    expect(probePreflight({ state: "ready", capabilities: [] }).kind).toBe("ok");
    expect(probePreflight({ state: "ready" }).kind).toBe("ok");
  });

  it("识别「命令没注册」，因为它要的是重启 App 而不是重连设备", () => {
    expect(looksLikeMissingCommand("Command device_binary_probe not found")).toBe(true);
    expect(looksLikeMissingCommand("Unknown command: `device_binary_probe`")).toBe(true);
    expect(looksLikeMissingCommand("命令 device_binary_probe 不存在")).toBe(true);
    expect(looksLikeMissingCommand("hosted.probe 需要 Agent 在线")).toBe(false);
  });

  it("九条同样的失败归并成一条，不同的不归并", () => {
    expect(repeatedError(["需要 Agent 在线", "需要 Agent 在线", "需要 Agent 在线"])).toBe(
      "需要 Agent 在线",
    );
    expect(repeatedError(["a", "b"])).toBeNull();
    expect(repeatedError(["只剩一条", undefined])).toBeNull();
    expect(repeatedError([undefined, undefined])).toBeNull();
  });
});
