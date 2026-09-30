/**
 * 设备 shell 宿主（FT2 / H-2）的参数拼装。
 *
 * 这里钉的是两件事：**别猜 adb 在哪**（没配就是没配，说清楚去设置里配），
 * 以及**别把坏字符拼进参数数组**。`-tt` 那两个 t 的含义与三条实测结论写在 `deviceShell.ts` 顶上。
 */
import { describe, expect, it } from "vitest";

import { deviceShellLaunch } from "./deviceShell";

describe("设备 shell 启动参数（H-2）", () => {
  it("拼出来的是 adb -s <serial> shell -tt", () => {
    const built = deviceShellLaunch("/Users/me/Library/Android/sdk/platform-tools/adb", "18271FDF600FL4");
    expect(built.ok).toBe(true);
    if (!built.ok) return;
    expect(built.launch.program).toBe("/Users/me/Library/Android/sdk/platform-tools/adb");
    expect(built.launch.args).toEqual(["-s", "18271FDF600FL4", "shell", "-tt"]);
  });

  /** 无线调试的序列号本身就是 `ip:port`，白名单不许把它当坏字符拒掉。 */
  it("ip:port 形式的序列号是合法的", () => {
    const built = deviceShellLaunch("/usr/bin/adb", "192.168.1.9:5555");
    expect(built.ok && built.launch.args).toEqual(["-s", "192.168.1.9:5555", "shell", "-tt"]);
  });

  /** 没配 adb 时**不猜**一个 "adb" 去 PATH 里找：那种"能跑就行"的猜法会把
   * 「工具检测里的 adb」和「实际用的 adb」拆成两个东西，版本不一致时查不动。 */
  it("没有 adb 路径 ⇒ no-adb，不去猜", () => {
    expect(deviceShellLaunch("", "SERIAL")).toEqual({ ok: false, block: "no-adb" });
    expect(deviceShellLaunch(null, "SERIAL")).toEqual({ ok: false, block: "no-adb" });
    expect(deviceShellLaunch("   ", "SERIAL")).toEqual({ ok: false, block: "no-adb" });
  });

  it("没有在线设备 ⇒ no-serial，界面据此说开不了", () => {
    expect(deviceShellLaunch("/usr/bin/adb", null)).toEqual({ ok: false, block: "no-serial" });
    expect(deviceShellLaunch("/usr/bin/adb", "  ")).toEqual({ ok: false, block: "no-serial" });
  });

  /** 参数数组是直接 exec 的（不进 shell 分词），但白名单仍要留：
   * 序列号来自设备列表，一旦被换成手输的值，坏字符就直接成了远端命令的一部分。 */
  it("坏序列号一律拒绝：空格、分号、换行、超长", () => {
    for (const bad of ["a b", "x;rm -rf /", "line1\nline2", "../etc", "A".repeat(65)]) {
      expect(deviceShellLaunch("/usr/bin/adb", bad)).toEqual({ ok: false, block: "bad-serial" });
    }
  });
});
