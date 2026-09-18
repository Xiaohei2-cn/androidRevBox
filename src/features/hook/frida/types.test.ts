import { describe, expect, it } from "vitest";
import type { FridaEvent } from "@/api/hook";
import {
  DEFAULT_FILTER,
  filterMatch,
  filterStorageKey,
  loadFilter,
  saveFilter,
  remoteSpec,
  type ConsoleEvent,
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
