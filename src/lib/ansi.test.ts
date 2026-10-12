import { describe, expect, it } from "vitest";

import { stripAnsi } from "./ansi";

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
