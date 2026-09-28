import { describe, expect, it } from "vitest";
import type { FridaEvent } from "@/api/hook";
import {
  buildConsoleExport,
  DEFAULT_FILTER,
  exportFileName,
  filterMatch,
  formatBytes,
  filterStorageKey,
  loadFilter,
  saveFilter,
  remoteSpec,
  fridaSessionLabel,
  type ConsoleEvent,
  type FridaFilter,
} from "./types";

function evt(over: Partial<ConsoleEvent>): ConsoleEvent {
  const e: Partial<ConsoleEvent> = { id: 1, ts: 0, stream: "stdout", line: "", ...over };
  return e as ConsoleEvent;
}

const send: FridaEvent = { kind: "send", data: { sign: "e10a" } };
const log: FridaEvent = { kind: "log", lvl: "log", msg: "hello" };

describe("filterMatch（§5.3-M1）", () => {
  it("类型开关", () => {
    expect(filterMatch(evt({ evt: send, line: "x" }), DEFAULT_FILTER)).toBe(true);
    expect(filterMatch(evt({ evt: send, line: "x" }), { ...DEFAULT_FILTER, send: false })).toBe(false);
    expect(filterMatch(evt({ evt: log, line: "x" }), { ...DEFAULT_FILTER, log: false })).toBe(false);
  });

  it("关键词大小写不敏感", () => {
    const f = { ...DEFAULT_FILTER, kw: "SIGN" };
    expect(filterMatch(evt({ evt: send, line: 'sign("abc")' }), f)).toBe(true);
    expect(filterMatch(evt({ evt: log, line: "nope" }), f)).toBe(false);
  });

  it("正则开关与坏模式退化", () => {
    const f = { ...DEFAULT_FILTER, kw: "^sign", regex: true };
    expect(filterMatch(evt({ evt: log, line: "sign ok" }), f)).toBe(true);
    expect(filterMatch(evt({ evt: log, line: "x sign" }), f)).toBe(false);
    // 坏正则 → 关键词包含兜底不抛
    expect(filterMatch(evt({ evt: log, line: "a(b" }), { ...f, kw: "a(b" })).toBe(true);
  });

  it("ready/exit/raw 归入「其他」开关", () => {
    const f = { ...DEFAULT_FILTER, raw: false };
    expect(filterMatch(evt({ evt: { kind: "ready", pid: 1, mode: "spawn", pkg: "x" }, line: "" }), f)).toBe(false);
    expect(filterMatch(evt({ evt: { kind: "exit", code: 0 }, line: "" }), f)).toBe(false);
  });
});

describe("过滤持久化（localStorage，按设备+包）", () => {
  it("存/取/合并默认", () => {
    const key = filterStorageKey({ deviceSerial: "s1", target: "com.x" });
    saveFilter(key, { ...DEFAULT_FILTER, kw: "sign" });
    const back = loadFilter(key);
    expect(back.kw).toBe("sign");
    expect(back.send).toBe(true);
  });
  it("坏 JSON 回退默认", () => {
    localStorage.setItem("bad-key", "{oops");
    expect(loadFilter("bad-key")).toEqual(DEFAULT_FILTER);
  });
});

describe("remoteSpec", () => {
  it("USB 不探远程；远程空端口回 27042", () => {
    expect(remoteSpec({ connMode: "usb", port: "99" })).toBeUndefined();
    expect(remoteSpec({ connMode: "remote", port: "" })).toBe("127.0.0.1:27042");
    expect(remoteSpec({ connMode: "remote", port: "27043" })).toBe("127.0.0.1:27043");
  });
});

describe("fridaSessionLabel（attach 空目标 = 自动前台）", () => {
  it("attach 留空不报空、也不留空位：写成「自动前台」", () => {
    expect(fridaSessionLabel("attach", "", "01hook_tcp.js")).toBe(
      "attach 自动前台 · 01hook_tcp.js",
    );
    expect(fridaSessionLabel("attach", "   ", "a.js")).toBe("attach 自动前台 · a.js");
  });

  it("填了目标就用目标；spawn 的包名原样带出", () => {
    expect(fridaSessionLabel("attach", " com.x.y ", "a.js")).toBe("attach com.x.y · a.js");
    expect(fridaSessionLabel("spawn", "com.x.y", "a.js")).toBe("spawn com.x.y · a.js");
  });
});

describe("filterMatch 按看得见的文字过滤", () => {
  const evt = (line: string): ConsoleEvent => ({
    id: 1,
    ts: 0,
    stream: "stdout",
    line,
    evt: { kind: "log", lvl: "log", msg: line },
  });
  const f = (kw: string): FridaFilter => ({ ...DEFAULT_FILTER, kw });

  it("带 ANSI 颜色的 hexdump 也能按内容搜到（以前搜 00000000 是空的）", () => {
    const line = "\u001b[0;32m00000000\u001b[0m  \u001b[0;33m00\u001b[0m";
    expect(filterMatch(evt(line), f("00000000"))).toBe(true);
    expect(filterMatch(evt(line), f("0000ffff"))).toBe(false);
    // 正则模式同理：转义不该插进匹配串中间
    expect(filterMatch(evt(line), { ...f("0000.*00$"), regex: true })).toBe(true);
  });
});

describe("「保存日志」拼出来的文本（第七十四轮）", () => {
  const E = "\u001b";
  const mk = (
    ts: number,
    line: string,
    kind: "log" | "send" | "raw" = "log",
    stream: ConsoleEvent["stream"] = "stdout",
  ): ConsoleEvent => ({
    id: ts,
    ts,
    stream,
    line,
    evt: kind === "send" ? { kind, data: line } : { kind, msg: line } as never,
  });

  it("界面文本存的是**看得见的字**，颜色码不进文件", () => {
    const text = buildConsoleExport(
      [mk(1_760_000_000_000, `${E}[0;32m00000000${E}[0m hello`)],
      "text",
    );
    expect(text).not.toContain(E);
    expect(text).toContain("00000000 hello");
    // 行首要时间戳，用户拿文件对屏幕才有得对
    expect(text).toMatch(/^\d{2}:\d{2}:\d{2}\.\d{3} \[stdout\] /);
  });

  it("原始行格式逐行留底，连 NDJSON 原文与转义都不动（排查协议用）", () => {
    const wire = JSON.stringify({ t: "send", data: "abc" });
    expect(buildConsoleExport([mk(0, wire, "send")], "jsonl")).toBe(wire + "\n");
  });

  it("CSV 把逗号、引号、换行都关进引号里，不然一行会变两列", () => {
    const out = buildConsoleExport(
      [mk(1_760_000_000_000, 'a,b "q"\nc'), mk(1_760_000_000_001, "plain")],
      "csv",
    );
    expect(out.split("\n")[0]).toBe("ts,stream,kind,text");
    expect(out).toContain('"a,b ""q""\nc"');
    expect(out).toContain(",stdout,log,plain");
  });

  it("顺序 = 到达顺序，一行不多一行不少（过滤是调用方的事，这里不重排）", () => {
    const events = [mk(30, "c"), mk(10, "a"), mk(20, "b")];
    const lines = buildConsoleExport(events, "jsonl").trim().split("\n");
    expect(lines).toEqual(["c", "a", "b"]);
  });

  it("默认文件名里不许有路径分隔符：带着 / 的名字会让保存框落到不存在的目录", () => {
    const name = exportFileName('spawn com.x/lib.so · "a\\b.js"', "text");
    expect(name).not.toContain("/");
    expect(name).not.toContain("\\");
    expect(name.endsWith(".log")).toBe(true);
    expect(exportFileName("   ", "csv")).toMatch(/^frida-.*\.csv$/);
  });

  it("字节数说人话", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});
