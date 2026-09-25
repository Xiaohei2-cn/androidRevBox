import { describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { hookApi, parseFridaLine, summarizeEventData } from "@/api/hook";

describe("parseFridaLine（runner NDJSON 协议，§5.2）", () => {
  it("ready 控制行", () => {
    expect(
      parseFridaLine('{"t":"ready","pid":1234,"mode":"spawn","pkg":"com.x","frida":"16.5.9"}'),
    ).toEqual({ kind: "ready", pid: 1234, mode: "spawn", pkg: "com.x", frida: "16.5.9" });
  });

  it("log 三级", () => {
    expect(parseFridaLine('{"t":"log","lvl":"warn","msg":"slow"}')).toEqual({
      kind: "log",
      lvl: "warn",
      msg: "slow",
    });
  });

  it("send 带 tag/seq 与二进制旁路", () => {
    const e = parseFridaLine('{"t":"send","tag":"sign","seq":7,"data":{"a":1},"data_bin":{"_hex":"1a2b"}}');
    expect(e).toEqual({ kind: "send", tag: "sign", seq: 7, data: { a: 1 }, dataBin: "1a2b" });
  });

  it("error 行带堆栈", () => {
    expect(parseFridaLine('{"t":"error","why":"boom","stack":"at x"}')).toEqual({
      kind: "error",
      why: "boom",
      stack: "at x",
    });
  });

  it("exit 行", () => {
    expect(parseFridaLine('{"t":"exit","code":0,"why":"process-terminated"}')).toEqual({
      kind: "exit",
      code: 0,
      why: "process-terminated",
    });
  });

  it("非协议行降级原文不崩", () => {
    for (const line of ["plain text", "pid=42", "{not json", "[1,2]", '{"t":"future","x":1}', ""]) {
      expect(parseFridaLine(line)).toEqual({ kind: "raw", text: line });
    }
  });

  it("pid 非数字（attach 无 pid）容忍", () => {
    const e = parseFridaLine('{"t":"ready","mode":"attach","pkg":"com.x"}');
    expect(e.kind === "ready" && e.pid).toBe(null);
  });
});

describe("summarizeEventData", () => {
  it("标量与对象", () => {
    expect(summarizeEventData("abc")).toBe("abc");
    expect(summarizeEventData(42)).toBe("42");
    expect(summarizeEventData(null)).toBe("null");
    expect(summarizeEventData({ a: 1 })).toBe('{"a":1}');
  });
});

describe("hook_preflight 的入参", () => {
  it("USB 模式必须把 serial 一起送出去（否则探的是上一台机的 frida-server）", async () => {
    invokeMock.mockReset().mockResolvedValue({});
    await hookApi.preflight(undefined, "18271FDF600FL4");
    expect(invokeMock).toHaveBeenCalledWith("hook_preflight", {
      remote: null,
      serial: "18271FDF600FL4",
    });
  });

  it("远程模式给 endpoint，serial 留空", async () => {
    invokeMock.mockReset().mockResolvedValue({});
    await hookApi.preflight("127.0.0.1:27042");
    expect(invokeMock).toHaveBeenCalledWith("hook_preflight", {
      remote: "127.0.0.1:27042",
      serial: null,
    });
  });
});
