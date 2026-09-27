import { describe, expect, it } from "vitest";

import {
  hostedBaseName,
  isSafeHostedName,
  planHostedUpload,
  rejectReasonText,
} from "./hostedUpload";

const none = { names: new Set<string>(), runningNames: new Set<string>() };

describe("托管文件名白名单（与 Rust is_safe_hosted_name 同形）", () => {
  it("放行常规二进制名", () => {
    for (const name of ["frida-server", "auth-server", "x86_64.so", "a_b-1.JS"]) {
      expect(isSafeHostedName(name), name).toBe(true);
    }
  });

  it("拦住白名单外的名字（这些推上去也不会出现在托管列表）", () => {
    for (const name of ["", ".hidden", "带空格 x", "中文", "a;b", "a/b", "a\\b", "a=b"]) {
      expect(isSafeHostedName(name), name).toBe(false);
    }
    expect(isSafeHostedName("x".repeat(129))).toBe(false);
  });
});

describe("hostedBaseName", () => {
  it("posix 与 windows 分隔符都认", () => {
    expect(hostedBaseName("/Users/citec/Downloads/frida-server")).toBe("frida-server");
    expect(hostedBaseName("C:\\Users\\a\\frida-server")).toBe("frida-server");
    expect(hostedBaseName("  /tmp/x.js  ")).toBe("x.js");
  });

  it("目录（以分隔符结尾）拿不到文件名 → 交给上层拒绝，不硬拼", () => {
    expect(hostedBaseName("/tmp/somedir/")).toBe("");
  });
});

describe("planHostedUpload", () => {
  it("规划出目标路径，并标出覆盖与在跑", () => {
    const plan = planHostedUpload(
      ["/Users/x/Downloads/frida-server", "/Users/x/Downloads/auth-server"],
      { names: new Set(["auth-server"]), runningNames: new Set(["auth-server"]) },
    );
    expect(plan.rejected).toHaveLength(0);
    expect(plan.accepted[0]).toMatchObject({
      remote: "/data/local/tmp/frida-server",
      overwrite: false,
      running: false,
    });
    expect(plan.accepted[1]).toMatchObject({
      remote: "/data/local/tmp/auth-server",
      overwrite: true,
      running: true,
    });
  });

  it("非法名与目录被拒且带原因，合法的照常进", () => {
    const plan = planHostedUpload(["/tmp/中文", "/tmp/dir/", "/tmp/ok.bin"], none);
    expect(plan.accepted.map((i) => i.name)).toEqual(["ok.bin"]);
    expect(plan.rejected.map((r) => r.reason)).toEqual(["bad-name", "no-name"]);
    expect(rejectReasonText("bad-name")).toContain("白名单");
  });

  it("同一批重名只推第一个，第二个明确拒掉（不静默互相覆盖）", () => {
    const plan = planHostedUpload(["/a/x.bin", "/b/x.bin"], none);
    expect(plan.accepted).toHaveLength(1);
    expect(plan.rejected[0].reason).toBe("duplicate");
  });
});
