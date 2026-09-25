import { describe, expect, it } from "vitest";

import { ansiRuns, stripAnsi } from "./ansi";

const E = "\u001b";
/** 用户现场：脚本打的 hexdump 行（地址绿、零字节黄） */
const HEXDUMP =
  `${E}[0;32m00000000${E}[0m  ${E}[0;33m00${E}[0m ${E}[0;33m01${E}[0m ${E}[0;33m0c${E}[0m` +
  `  ${E}[0;33m.${E}[0m${E}[0;33m.${E}[0m`;

describe("stripAnsi", () => {
  it("吃掉 SGR、光标移动、清行与 OSC，只留看得见的文字", () => {
    expect(stripAnsi(HEXDUMP)).toBe("00000000  00 01 0c  ..");
    expect(stripAnsi(`${E}[2A`)).toBe("");
    expect(stripAnsi(`${E}[Kabc`)).toBe("abc");
    expect(stripAnsi(`${E}]8;;https://x\u0007link${E}]8;;\u0007`)).toBe("link");
    expect(stripAnsi("没有转义")).toBe("没有转义");
  });
});

describe("ansiRuns", () => {
  it("SGR 翻成样式，文字一个不丢", () => {
    const runs = ansiRuns(HEXDUMP);
    // 不变式：拼起来 === 去掉转义后的纯文本
    expect(runs.map((r) => r.text).join("")).toBe(stripAnsi(HEXDUMP));
    expect(runs[0]).toMatchObject({ text: "00000000", color: "text-emerald-400" });
    expect(runs.some((r) => r.color === "text-amber-300")).toBe(true);
    // 复位之后的普通空格不该挂着颜色
    expect(runs.filter((r) => r.text === "  ").every((r) => r.color === null)).toBe(true);
  });

  it("没有转义时就是原样一段，不产生额外样式", () => {
    expect(ansiRuns("[socket] fd=113")).toEqual([
      { text: "[socket] fd=113", color: null, bold: false },
    ]);
  });

  it("不认识的参数只丢样式、不丢文字；未闭合的尾巴也不吃掉内容", () => {
    expect(ansiRuns(`${E}[4m下划线${E}[0m`).map((r) => r.text).join("")).toBe("下划线");
    expect(ansiRuns(`${E}[38;5;196m红${E}[0m`).map((r) => r.text).join("")).toBe("红");
    expect(ansiRuns(`前半${E}[0;31m`).map((r) => r.text).join("")).toBe("前半");
    expect(ansiRuns(`尾段${E}[0;31m还在`).map((r) => r.text).join("")).toBe("尾段还在");
  });

  it("加粗能带出来（脚本常用 1;32 表示重点）", () => {
    const runs = ansiRuns(`${E}[1;32mRESUME${E}[0m`);
    expect(runs[0]).toMatchObject({ text: "RESUME", bold: true, color: "text-emerald-400" });
  });

  it("相邻同样式合并：一屏 hexdump 几十段颜色不该生成几十倍节点", () => {
    const line = `${E}[0;33m00${E}[0m${E}[0;33m01${E}[0m${E}[0;33m02${E}[0m`;
    expect(ansiRuns(line)).toHaveLength(1);
    expect(ansiRuns(line)[0].text).toBe("000102");
  });
});
