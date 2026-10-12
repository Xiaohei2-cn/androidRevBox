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
    },
  };
}

/**
 * 交互式托管会话：在**同一块设备 PTY 屏**里跑某个托管二进制（IDA 的 `id_android_svr64`、
 * `gdbserver`、`lldb-server`、任何"要边输出边等输入"的控制台程序）。
 *
 * 为什么这类程序不能走「托管执行」那条路（结构性不匹配，不是参数没调好）：
 *
 * | 托管执行（Agent nohup + 日志 + 可选 stdin 管道） | 这类程序要的是 |
 * |---|---|
 * | 没有 PTY：程序 `isatty()` 走非交互分支，不做行编辑、不回提示符 | 有 tty，提示符/回显/全屏都在 |
 * | 输出靠**轮询日志文件**，调试会话里等于"看录像" | 输出即时到屏上 |
 * | stdin 是"喂一行"，没有 Ctrl-C / Ctrl-D / 窗口尺寸 | 键盘与信号直达对端 |
 *
 * 所以这里只做一件事：把命令行拼成 `adb -s <serial> shell -tt "<远端命令>"`，
 * 交给设备那块屏（见本文件顶部的实测表 —— `-tt` 有 winsize 通道、Ctrl-C 走行规程）。
 * 不新增 Rust 通道，也不需要重装 Agent。
 *
 * 引用规则：**每个 token 单独 `shQuote`**，远端只看到一个字符串参数，不经过我们的 shell 分词。
 * `su -c` 那层是**再套一次引用**（内层字符串本身就是设备侧 shell 的一条命令），
 * 所以出现 `'\''` 这种嵌套转义是正常的，不是漏网。
 */
export type HostedBinaryBlock = DeviceShellBlock | "bad-name";

export interface HostedBinaryLaunchRequest {
  /** 托管目录（设备侧），如 `/data/local/tmp` */
  dir: string;
  /** 二进制文件名（不含目录） */
  name: string;
  /** 启动参数，逐个引用 */
  args?: string[];
  /** 以 root 起（`su -c`）：注入类 server 常态需要 */
  root?: boolean;
}

/** POSIX 单引号引用：内部出现的 `'` 收拢成 `'\''`，其余一律字面量。 */
export function shQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** 文件名不许带路径分隔与控制字符：目录由 `dir` 固定，这里只该是个名字。 */
const NAME_PATTERN = /^[^\s/\\]{1,120}$/;

export function hostedBinaryLaunch(
  adbPath: string | null | undefined,
  serial: string | null | undefined,
  request: HostedBinaryLaunchRequest,
): { ok: true; launch: DeviceShellLaunch; remote: string } | { ok: false; block: HostedBinaryBlock } {
  const base = deviceShellLaunch(adbPath, serial);
  if (!base.ok) return { ok: false, block: base.block };
  const name = request.name.trim();
  if (!NAME_PATTERN.test(name)) return { ok: false, block: "bad-name" };
  const dir = (request.dir || "/data/local/tmp").trim() || "/data/local/tmp";
  // 必须带 `./`：Android 的 shell 找可执行文件走 PATH，裸文件名会直接
  // `inaccessible or not found`（真机上报的就是这条）。名字里已含目录时尊重原样。
  const executable = name.includes("/") ? name : `./${name}`;
  const inner = [executable, ...(request.args ?? [])].map(shQuote).join(" ");
  // 远端由 `/system/bin/sh -c` 解析：先 cd 进托管目录，再（可选）换 root 执行
  const remote = request.root
    ? `cd ${shQuote(dir)} && su -c ${shQuote(inner)}`
    : `cd ${shQuote(dir)} && ${inner}`;
  return {
    ok: true,
    launch: { program: base.launch.program, args: [...base.launch.args, remote] },
    remote,
  };
}
