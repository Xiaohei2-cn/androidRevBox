/**
 * 设备 shell 宿主（FT2 / §4.2 的 H-2）：把 `adb -s <serial> shell -tt` 拼成一条能进 PTY 的命令。
 *
 * **这一条不需要新的 Rust 通道**：桌面那层 PTY 不关心跑的是什么程序，`adb` 也是程序。
 * 换宿主只是换"谁在喂这块屏"，渲染与会话管理完全复用（这是"用开源内核"换来的结构收益）。
 *
 * 三条结论是 2026-09-28 在 Pixel 6（`18271FDF600FL4`，Android 16）上用真 PTY 实测出来的，
 * 不是从文档抄的——其中一条**推翻了我自己原先的假设**（原来以为 `adb shell` 没有 winsize 通道）：
 *
 * | 实测项 | 结果 | 对设计的约束 |
 * |---|---|---|
 * | ① `-tt` 下按 Ctrl-C（远端 `trap … INT`） | 远端打出 `CAUGHT-INT` | 中断走终端行规程，不模拟、不代发 |
 * | ② 本地 `TIOCSWINSZ` 改成 12×40 后远端 `stty size` | `40 120` → `12 40` | **resize 有通道**，不需要"重开会话换尺寸" |
 * | ③ `echo TO_OUT; echo TO_ERR 1>&2` | 两路都在同一条 PTY 流里 | 混流是 PTY 语义；**不许**把这条流喂给任何按行解析的现有链路 |
 * | ④ 远端 `exit 7` | 本地 waitstatus → 退出码 7，stdout 里**没有** "exit code was 7" | 不用加 `-x`；退出码能如实落到会话状态 |
 * | ⑤ 不加 `-tt` 的对照 | Ctrl-C 也能到远端，但没有回显/行编辑/全屏程序 | 交互仍然必须 `-tt` |
 *
 * `-t` 出现两次是必须的：第一次给**远端**分配 PTY，第二次连**本地 stdin 不是 tty** 时也分配
 * （我们本地本来就是 PTY，写 `-tt` 是为了不管从哪儿起都同一个形状）。
 */

export type DeviceShellBlock = "no-adb" | "no-serial" | "bad-serial";

export interface DeviceShellLaunch {
  program: string;
  args: string[];
  /** 界面用来当 tab 标题：`adb shell @ <serial 尾 6 位>` */
  title: string;
}

/** serial 白名单：adb 的 `-s` 只接受这些字符，别把空格或 `;` 拼进参数数组。 */
const SERIAL_PATTERN = /^[A-Za-z0-9._:-]{1,64}$/;

export function deviceShellLaunch(
  adbPath: string | null | undefined,
  serial: string | null | undefined,
): { ok: true; launch: DeviceShellLaunch } | { ok: false; block: DeviceShellBlock } {
  const path = adbPath?.trim();
  if (!path) return { ok: false, block: "no-adb" };
  const id = serial?.trim();
  if (!id) return { ok: false, block: "no-serial" };
  // 参数数组是直接 exec 的，不做 shell 分词；但白名单仍然要留：
  // 调用方给的东西来自设备列表，一旦被换成手输的值，坏字符会直接变成远端命令的一部分
  if (!SERIAL_PATTERN.test(id)) return { ok: false, block: "bad-serial" };
  return {
    ok: true,
    launch: {
      program: path,
      args: ["-s", id, "shell", "-tt"],
      title: `adb shell @ ${id.length > 8 ? id.slice(-6) : id}`,
    },
  };
}
