/**
 * 本机记录（文件备注 + 启动参数）的按设备隔离与清理判据。
 *
 * 这一段代码会**删用户数据**，所以每条断言都对着一个真实的出错形状：
 * 换手机不能带走另一台的记录；清单不完整时一条都不许删；
 * 空目录/回退腿的清单不能当成"这台手机上没有这个文件"的证据。
 */
import { describe, expect, it } from "vitest";

import {
  LAUNCH_PREFIX,
  NOTE_PREFIX,
  launchKeyOf,
  listingIsTrustworthy,
  noteKey,
  orphanRecordNames,
  pickTrackedDevice,
  pruneHostedRecords,
} from "./hostedRecords";

/** 自己造一份存储：断言"动了哪些键"比断言界面文案更直接 */
function fakeStorage(entries: Record<string, string>) {
  const map = new Map(Object.entries(entries));
  return {
    map,
    get length() {
      return map.size;
    },
    key: (i: number) => [...map.keys()][i] ?? null,
    removeItem: (k: string) => {
      map.delete(k);
    },
  };
}

describe("hostedRecords：本机记录按设备号各管各的", () => {
  it("只删这台设备上、清单里已不存在的文件；别的设备一条不碰", () => {
    const storage = fakeStorage({
      [noteKey("PIXEL-1", "frida-server")]: "在跑 27042",
      [noteKey("PIXEL-1", "gone.bin")]: "旧备注",
      [`${launchKeyOf("PIXEL-1", "gone.bin")}.args`]: "-l 27042",
      [`${launchKeyOf("PIXEL-1", "gone.bin")}.stamp`]: "120|3000",
      [noteKey("OTHER-9", "gone.bin")]: "另一台手机的同名记录",
      "app.settings.theme": "dark",
    });
    const removed = pruneHostedRecords(storage, "PIXEL-1", new Set(["gone.bin"]));
    expect(removed.sort()).toEqual(
      [
        noteKey("PIXEL-1", "gone.bin"),
        `${launchKeyOf("PIXEL-1", "gone.bin")}.args`,
        `${launchKeyOf("PIXEL-1", "gone.bin")}.stamp`,
      ].sort(),
    );
    expect(storage.map.has(noteKey("OTHER-9", "gone.bin"))).toBe(true);
    expect(storage.map.has("app.settings.theme")).toBe(true);
    expect(storage.map.has(noteKey("PIXEL-1", "frida-server"))).toBe(true);
  });

  it("报告用而不删：orphanRecordNames 给出同样的名单，先看清再动手", () => {
    const storage = fakeStorage({
      [noteKey("PIXEL-1", "kept")]: "x",
      [noteKey("PIXEL-1", "gone-2")]: "y",
      [`${launchKeyOf("PIXEL-1", "gone-1")}.args`]: "z",
    });
    expect(orphanRecordNames(storage, "PIXEL-1", new Set(["kept"]))).toEqual([
      "gone-1",
      "gone-2",
    ]);
    expect(storage.length).toBe(3);
  });

  it("文件名可以带点（helper.dex），参数后缀不会把它切错", () => {
    const storage = fakeStorage({
      [`${launchKeyOf("PIXEL-1", "helper.dex")}.args`]: "-h",
      [`${launchKeyOf("PIXEL-1", "helper.dex")}.stamp`]: "1|2",
      [noteKey("PIXEL-1", "helper.dex")]: "系统类",
    });
    expect(orphanRecordNames(storage, "PIXEL-1", new Set(["helper.dex"]))).toEqual([]);
    // 名单（已确认被删的名字）里没有它，就一条都不动——哪怕清单一时没带上它
    expect(pruneHostedRecords(storage, "PIXEL-1", new Set())).toEqual([]);
    expect(pruneHostedRecords(storage, "PIXEL-1", new Set(["helper2"]))).toEqual([]);
  });

  it("形状不对的键（后缀不认识）留着不动，不猜它属于哪个文件", () => {
    const storage = fakeStorage({ [`${LAUNCH_PREFIX}PIXEL-1.gone.bin.bak`]: "?" });
    // 认不出后缀的键不属于任何文件：既不报成孤儿，也不删
    expect(orphanRecordNames(storage, "PIXEL-1", new Set())).toEqual([]);
    expect(pruneHostedRecords(storage, "PIXEL-1", new Set(["gone.bin"])).length).toBe(0);
    expect(storage.length).toBe(1);
  });

  it("前缀常量只有一个出口：写的一方与清的一方用的是同一串", () => {
    expect(noteKey("A1", "x")).toBe(`${NOTE_PREFIX}A1.x`);
    expect(launchKeyOf("A1", "x")).toBe(`${LAUNCH_PREFIX}A1.x`);
  });
});

describe("listingIsTrustworthy：谁有资格说「这台手机上没这个文件」", () => {
  const base = { binaries: [{ name: "a" }], truncated: false, unreadable: [], source: "agent" };

  it("Agent 的完整清单才有资格", () => {
    expect(listingIsTrustworthy(base)).toBe(true);
  });

  it("回退腿、被截断、有读不出的条目、空目录：四种都不许据此删记录", () => {
    for (const bad of [
      { source: "adb" },
      { truncated: true },
      { unreadable: ["weird.bin"] },
      { binaries: [] },
    ]) {
      expect(listingIsTrustworthy({ ...base, ...bad }), JSON.stringify(bad)).toBe(false);
    }
  });
});

describe("pickTrackedDevice：界面盯着哪台设备", () => {
  it("adb 报了别的设备在线才改选，并把「这是换机」说出来", () => {
    expect(pickTrackedDevice("OLD-1", ["NEW-1", "NEW-2"])).toEqual({
      serial: "NEW-1",
      switchedAway: true,
    });
  });

  it("枚举到空列表时保持原选择：换线的空窗期不该收掉用户正在用的屏", () => {
    expect(pickTrackedDevice("OLD-1", [])).toEqual({ serial: "OLD-1", switchedAway: false });
  });

  it("选中的设备还在线就什么都不动；从没选过就落到第一台（那不算换机）", () => {
    expect(pickTrackedDevice("PIXEL-1", ["PIXEL-1"])).toEqual({
      serial: "PIXEL-1",
      switchedAway: false,
    });
    expect(pickTrackedDevice(null, ["PIXEL-1"])).toEqual({
      serial: "PIXEL-1",
      switchedAway: false,
    });
  });

  it("一台设备都没有且本来就没选：还是 null，不去调任何接口", () => {
    expect(pickTrackedDevice(null, [])).toEqual({ serial: null, switchedAway: false });
  });
});
