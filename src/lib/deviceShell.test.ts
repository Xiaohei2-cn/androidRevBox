/**
 * 设备 shell 宿主（FT2 / H-2）的参数拼装。
 *
 * 这里钉的是两件事：**别猜 adb 在哪**（没配就是没配，说清楚去设置里配），
 * 以及**别把坏字符拼进参数数组**。`-tt` 那两个 t 的含义与三条实测结论写在 `deviceShell.ts` 顶上。
 */
import { describe, expect, it } from "vitest";

import { deviceShellLaunch, hostedBinaryLaunch } from "./deviceShell";

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

describe("hostedBinaryLaunch：给交互调试会话拼远端命令", () => {
  const r = (over: Partial<Parameters<typeof hostedBinaryLaunch>[2]>) =>
    hostedBinaryLaunch("/sdk/adb", "PIXEL-1", {
      dir: "/data/local/tmp",
      name: "id_android_svr64",
      ...over,
    });

  it("形状是 adb -s <serial> shell -tt <远端串>，每个 token 各自引用", () => {
    const out = r({ args: ["-p", "2345"] });
    if (!out.ok) throw new Error("应当成功");
    expect(out.launch.args).toEqual(["-s", "PIXEL-1", "shell", "-tt", out.remote]);
    // `./` 是必须的：Android shell 按 PATH 找裸文件名，会报 inaccessible or not found
    expect(out.remote).toBe(`cd '/data/local/tmp' && './id_android_svr64' '-p' '2345'`);
  });

  /** 设备侧 shell 怎么读这个串，测试就得怎么还原：POSIX 单引号文法里 `'\''` 才是一个引号 */
  function unquote(text: string): string {
    if (!text.startsWith("'") || !text.endsWith("'")) {
      throw new Error(`不是一个单引号串：${text}`);
    }
    // 单引号内一切都是字面量，唯一的转义写法是：收尾 + 一个反斜杠引号 + 重开
    return text.slice(1, -1).split("'\\''").join("'");
  }

  it("root：su -c 再套一层引用，设备 shell 解析一次后内层命令原样还原", () => {
    const out = r({ args: ["it work's"], root: true });
    if (!out.ok) throw new Error("应当成功");
    const m = /^cd '[^']*' && su -c (.+)$/.exec(out.remote);
    expect(m, out.remote).toBeTruthy();
    // 还原出来仍是"每个 token 各自引用"的一条命令，参数里的引号没把 su -c 提前关掉
    expect(unquote(m![1])).toBe("'./id_android_svr64' 'it work'\\''s'");
  });

  it("参数里的 ; 与 $ 只是字面量：它们待在引号里", () => {
    const out = r({ args: ["a; rm -rf /", "$(id)"] });
    if (!out.ok) throw new Error("应当成功");
    expect(out.remote).toContain(`'a; rm -rf /'`);
    expect(out.remote).toContain(`'$(id)'`);
    // 除了我们自己拼的那个分隔符，远端串里不该有第二个裸 &&
    expect(out.remote.match(/&&/g)?.length).toBe(1);
  });

  it("名字里已经带目录就不再补 ./", () => {
    const out = hostedBinaryLaunch("/sdk/adb", "PIXEL-1", { dir: "/d", name: "sub/x" , args: [] });
    // sub/x 含斜杠：NAME_PATTERN 允许吗？不允许（拒绝带路径的名字）
    expect(out).toEqual({ ok: false, block: "bad-name" });
  });

  it("名字带路径或空格一律拒绝（目录由 dir 固定）", () => {
    expect(r({ name: "sub/dir" })).toEqual({ ok: false, block: "bad-name" });
    expect(r({ name: "a b" })).toEqual({ ok: false, block: "bad-name" });
    expect(r({ name: "" })).toEqual({ ok: false, block: "bad-name" });
  });

  it("adb / serial 的闸沿用设备 shell 那一条，不另立口径", () => {
    expect(hostedBinaryLaunch("", "S", { dir: "/d", name: "x" })).toEqual({ ok: false, block: "no-adb" });
    expect(hostedBinaryLaunch("/adb", null, { dir: "/d", name: "x" })).toEqual({
      ok: false,
      block: "no-serial",
    });
    expect(hostedBinaryLaunch("/adb", "a b; rm", { dir: "/d", name: "x" })).toEqual({
      ok: false,
      block: "bad-serial",
    });
  });
});
