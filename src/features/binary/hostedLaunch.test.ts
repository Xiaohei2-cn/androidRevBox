import { beforeEach, describe, expect, it } from "vitest";
import type { HostedProbeResult } from "@/api/device";
import {
  HELP_CANDIDATES,
  MAX_ARG_LEN,
  MAX_ARGS,
  argsProblem,
  probeCategoryKey,
  classifyProbe,
  compareStamp,
  decodeStamp,
  describeProbeFacts,
  emptyLaunchPrefs,
  encodeStamp,
  loadLaunchPrefs,
  probeHasOutput,
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

  it("分类键都在词典口径里（防止加了一类却没配文案）", () => {
    expect(probeCategoryKey(probe({ timed_out: true }))).toMatch(/^adb\.binary\.probe\.cat/);
  });

  it("一句人话里要带得上耗时、码、字节数与截断", () => {
    const text = describeProbeFacts(probe({ elapsed_ms: 4_000, timed_out: true, killed: true, truncated: true }));
    expect(text).toContain("4000 ms");
    expect(text).toContain("超时已杀");
    expect(text).toContain("回传已截断");
    expect(describeProbeFacts(probe({ started: false, detail: "not_found: 文件不存在" }))).toContain(
      "无法执行",
    );
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
    saveLaunchPrefs("SERIAL1", "toybox", {
      argsText: "-l 27042",
      stdinText: "",
      interactive: true,
      stamp: "100:5",
    });
    expect(localStorage.getItem("adb.binary.launch.SERIAL1.toybox.args")).toBe("-l 27042");
    expect(localStorage.getItem("adb.binary.launch.SERIAL1.toybox.stdin")).toBeNull();
    expect(loadLaunchPrefs("SERIAL1", "toybox")).toEqual({
      argsText: "-l 27042",
      stdinText: "",
      interactive: true,
      stamp: "100:5",
    });
    // 换一台设备不该串味
    expect(loadLaunchPrefs("OTHER", "toybox")).toEqual(emptyLaunchPrefs());
    // 用户清空后读回来仍是空，且不留三个空键
    saveLaunchPrefs("SERIAL1", "toybox", emptyLaunchPrefs());
    expect(localStorage.getItem("adb.binary.launch.SERIAL1.toybox.args")).toBeNull();
    expect(loadLaunchPrefs("SERIAL1", "toybox")).toEqual(emptyLaunchPrefs());
  });
});
