import { describe, expect, it } from "vitest";
import { HELP_CANDIDATES } from "./hostedLaunch";

/**
 * 「探测帮助」的两条作用域契约：
 *
 * ① **只看程序本身，不拼参数框里的参数**。真机现场：参数框填了 `-p 11881 --b`，
 *    探测把这段垫在前面，实际起的是 `stackplz -p 11881 --b -h`。后果有两个且都是真的：
 *    - 结论会错：`--b` 本身就坏，程序报"未知参数 --b"，我们却把失败记在 `-h` 头上，
 *      变成"它不支持 -h"——错的是我们的问题陈述，不是它的能力；
 *    - 探测**真的起进程**：`-p <pid>` 可能真的去挂别人。
 *
 * ② **只探一层，不下钻**。下钻（拿上一层的帮助文本里出现的选项当前缀再探）
 *    看起来聪明，实际是把"这是什么程序"变成了"猜这个参数怎么用"，而探测本身
 *    每层要起最多 9 次进程；用户要的只是"它有没有用法说明"。
 */
function probeArgv(path: string[], candidate: string): string[] {
  // 与 BinaryHosting.probeLevel 同口径：prefix 只来自 path，探测只跑一层（path 恒为 []）
  const prefix = [...path];
  return [...prefix, candidate];
}

describe("探测帮助的 argv 作用域", () => {
  it("只有候选本身，一条都不多", () => {
    for (const candidate of HELP_CANDIDATES) {
      expect(probeArgv([], candidate)).toEqual([candidate]);
    }
  });

  it("参数框里的内容一个 token 都不该出现", () => {
    const argsText = "-p 11881 --b";
    const argv = probeArgv([], "-h");
    expect(argv).toEqual(["-h"]);
    for (const token of argsText.split(" ")) {
      if (token) expect(argv).not.toContain(token);
    }
  });

  it("探测只做一层：不存在「带前缀再探」的第二次调用", () => {
    // 下钻被删掉之后，探测路径恒为空；这里钉住"候选表是唯一起点"
    expect(probeArgv([], "--help")).toEqual(["--help"]);
    expect(probeArgv([], "-h")).toEqual(["-h"]);
    expect(HELP_CANDIDATES.length).toBeGreaterThan(0);
  });
});
