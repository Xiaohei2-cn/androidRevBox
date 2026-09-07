import { describe, expect, it } from "vitest";
import { tokenizeCommand } from "@/api/task";

describe("tokenizeCommand", () => {
  it("基础拆分", () => {
    expect(tokenizeCommand("ping -c 4 127.0.0.1")).toEqual({
      executable: "ping",
      args: ["-c", "4", "127.0.0.1"],
    });
  });

  it("多余空白与制表符", () => {
    expect(tokenizeCommand("  ls   -la\t/tmp ")).toEqual({
      executable: "ls",
      args: ["-la", "/tmp"],
    });
  });

  it("双引号包裹含空格参数", () => {
    expect(tokenizeCommand('adb shell "input text hello world"')).toEqual({
      executable: "adb",
      args: ["shell", "input text hello world"],
    });
  });

  it("单引号支持", () => {
    expect(tokenizeCommand("sh -c 'echo hi'")).toEqual({
      executable: "sh",
      args: ["-c", "echo hi"],
    });
  });

  it("引号内不拆分通配与管道字符（无 shell 展开）", () => {
    expect(tokenizeCommand('grep "a|b" log.txt')).toEqual({
      executable: "grep",
      args: ["a|b", "log.txt"],
    });
  });

  it("空输入返回 null", () => {
    expect(tokenizeCommand("")).toBeNull();
    expect(tokenizeCommand("   ")).toBeNull();
  });

  it("未闭合引号按已解析部分处理", () => {
    expect(tokenizeCommand('echo "unclosed')).toEqual({
      executable: "echo",
      args: ["unclosed"],
    });
  });

  it("中文参数保持完整", () => {
    expect(tokenizeCommand('echo "你好 世界"')).toEqual({
      executable: "echo",
      args: ["你好 世界"],
    });
  });
});
