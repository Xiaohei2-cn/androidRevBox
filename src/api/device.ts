import { invokeCommand } from "./client";
import { listenEvent } from "./events";
import type { TaskStatusPayload } from "./task";

/** 与 Rust AdbEnvironment 对齐（version 为 flatten：version/build 平铺在顶层） */
export interface AdbEnvironment {
  installed: boolean;
  path?: string;
  source?: "config" | "android_home" | "sdk_root" | "path_env" | "mock";
  version?: string;
  build?: string;
  hint?: string;
  probeError?: string;
}

export interface DeviceEntry {
  serial: string;
  state: string;
  transport: string;
  model: string;
}

export interface DeviceInfo {
  model: string;
  manufacturer: string;
  androidVersion: string;
  sdkInt: string;
  serial: string;
  /** wlan0 IPv4（未连 Wi-Fi / 读取失败为 null） */
  ip?: string | null;
}

export interface FileEntry {
  name: string;
  isDir: boolean;
  size: number;
  symlink?: string | null;
  /** `ls -l` 风格权限串（如 `-rw-rw-rw-`、`drwxr-xr-x`、`-rwsr-xr-x`）；Legacy 解析失败时为空 */
  perms?: string;
}

/** 设备端文件类型（AR7.1 `filesystem.*`，由 lstat 判定，不靠猜） */
export type FileKind =
  | "dir"
  | "file"
  | "symlink"
  | "socket"
  | "fifo"
  | "block"
  | "char"
  | "other";

export interface FileStat {
  name: string;
  kind: FileKind;
  /** 权限位数值（含 setuid/setgid/sticky，不含文件类型位），如 0o755 = 493 */
  mode: number;
  modeText: string;
  uid: number;
  gid: number;
  size: number;
  /** Unix epoch 秒；固定单位与时区，前端自己格式化 */
  mtimeUnix: number;
  symlinkTarget?: string | null;
  /** 当前 Agent 身份能否读内容（目录=能否列举）。false 时不要把 size=0 当成空文件 */
  readable: boolean;
}

export interface FileStatResult {
  /** 调用方原始输入 */
  requestedPath: string;
  /** 规范化（符号链接已解析）后的真实路径 */
  path: string;
  stat: FileStat;
}

/** 托管运行记录（AR7.2 `hosted.list` 的 runs）：身份是 pid + start time */
export type HostedRunState = "running" | "exited" | "unknown";

export interface HostedRunRecord {
  handle: string;
  name: string;
  pid: number;
  /** `/proc/<pid>/stat` 第 22 字段；PID 被复用时它必然不同 */
  startTimeTicks: number;
  startedAtUnix: number;
  logPath: string;
  root: boolean;
  state: HostedRunState;
  /** 只有 Agent 亲自启动并已回收的进程才有退出码 */
  exitCode?: number | null;
  detail?: string | null;
}

export type PreviewEncoding = "utf8" | "hex";

export interface FilePreviewResult {
  path: string;
  size: number;
  /** 本次内容在文件中的起始偏移（尾读时非 0） */
  offset: number;
  returnedBytes: number;
  encoding: PreviewEncoding;
  text?: string | null;
  /** 含 NUL 或非法 UTF-8 时给小写十六进制，不做 lossy 文本转换 */
  hex?: string | null;
  truncated: boolean;
  detail?: string | null;
}

/** 一条端口转发规则（adb forward --list 行） */
export interface ForwardRule {
  serial: string;
  local: string;
  remote: string;
}

/** /data/local/tmp 下被托管的 ELF 二进制 */
export interface HostedBinary {
  name: string;
  path: string;
  size: number;
  perms: string;
  hasExec: boolean;
}

/** 端口→PID 反查的持有进程行 */
export interface PortHolder {
  pid: number;
  name: string;
}

/** 托管进程的一个 LISTEN 端口（/proc/net/tcp(6) 十六进制已还原） */
export interface ListenPort {
  address: string;
  port: number;
  listen: boolean;
  family: "tcp" | "tcp6";
}

export interface DeviceChangedPayload {
  serial: string;
  transport: string;
  present: boolean;
  state: string;
  lastSeen: number;
}

export const deviceApi = {
  environment(): Promise<AdbEnvironment> {
    return invokeCommand<AdbEnvironment>("adb_environment");
  },
  /** 设置/清空 adb 路径，返回重新探测后的环境 */
  setPath(path: string): Promise<AdbEnvironment> {
    return invokeCommand<AdbEnvironment>("adb_set_path", { path });
  },
  list(readyOnly = false): Promise<DeviceEntry[]> {
    return invokeCommand<DeviceEntry[]>("devices_list", { args: { readyOnly } });
  },
  info(serial: string): Promise<DeviceInfo> {
    return invokeCommand<DeviceInfo>("device_info", { serial });
  },
  ls(serial: string, path: string): Promise<FileEntry[]> {
    return invokeCommand<FileEntry[]>("device_ls", { serial, path });
  },
  /**
   * 单路径元数据（AR7.1，Agent only）。`followSymlink=false` 是 lstat 语义（链接本身），
   * true 时取解析后的目标；路径含 `..` 或不在允许范围内会被设备端结构化拒绝。
   */
  fileStat(
    serial: string,
    path: string,
    followSymlink = false,
  ): Promise<FileStatResult> {
    return invokeCommand<FileStatResult>("device_file_stat", {
      serial,
      path,
      followSymlink,
    });
  },
  /**
   * 受限预览（AR7.1，Agent only）：文本按 utf8、二进制按 hex 返回；
   * `fromEnd=true` 为日志尾读语义。字节上限由设备端夹住（默认 64 KiB，最大 256 KiB）。
   */
  filePreview(
    serial: string,
    path: string,
    options?: { maxBytes?: number; fromEnd?: boolean },
  ): Promise<FilePreviewResult> {
    return invokeCommand<FilePreviewResult>("device_file_preview", {
      serial,
      path,
      maxBytes: options?.maxBytes ?? null,
      fromEnd: options?.fromEnd ?? false,
    });
  },
  packages(serial: string): Promise<string[]> {
    return invokeCommand<string[]>("device_packages", { serial });
  },
  /** 设备 wlan0 IPv4（adb -s <serial> shell ip addr show wlan0） */
  ip(serial: string): Promise<string | null> {
    return invokeCommand<string | null>("device_ip", { serial });
  },
  /** 设备 Root 状态（su -c id → uid=0；设备信息卡横幅用） */
  rootCheck(serial: string): Promise<boolean> {
    return invokeCommand<boolean>("device_root_check", { serial });
  },
  /** 建立端口转发（adb -s <serial> forward <local> <remote>） */
  forwardSetup(serial: string, local: string, remote: string): Promise<ForwardRule> {
    return invokeCommand<ForwardRule>("adb_forward_setup", { serial, local, remote });
  },
  /** 列出该设备当前全部转发规则 */
  forwardList(serial: string): Promise<ForwardRule[]> {
    return invokeCommand<ForwardRule[]>("adb_forward_list", { serial });
  },
  /** 删除转发规则（local 缺省 = 删全部） */
  forwardRemove(serial: string, local?: string): Promise<void> {
    return invokeCommand<void>("adb_forward_remove", { serial, local: local ?? null });
  },
  /** 列出 /data/local/tmp 下的 ELF 二进制（含执行权限） */
  binaries(serial: string): Promise<HostedBinary[]> {
    return invokeCommand<HostedBinary[]>("device_binaries", { serial });
  },
  /** 探测 su 可用性（Root 开关前置检查） */
  binarySuCheck(serial: string): Promise<boolean> {
    return invokeCommand<boolean>("device_binary_su_check", { serial });
  },
  /** 赋予执行权限（chmod +x；root=true 走 su -c） */
  binaryChmod(serial: string, name: string, root = false): Promise<void> {
    return invokeCommand<void>("device_binary_chmod", { serial, name, root });
  },
  /** 后台启动二进制，返回 pid（root=true 走 su -c） */
  binaryRun(serial: string, name: string, root = false): Promise<number> {
    return invokeCommand<number>("device_binary_run", { serial, name, root });
  },
  /** 托管运行表（AR7.2，Agent only）：真实运行状态与稳定句柄 */
  hostedRuns(serial: string): Promise<HostedRunRecord[]> {
    return invokeCommand<HostedRunRecord[]>("device_hosted_runs", { serial });
  },
  /**
   * 终止进程（AR6.3 写操作）。默认走 Agent process.kill：设备端发信号前会重读
   * /proc/<pid> 身份，`expectedName` 与实到身份不符时以 precondition_failed
   * 拒止（防 PID 复用误杀），所以列表里拿到名字就要带上。
   * root=true 仍走 Legacy su -c；Agent 不可用时不自动回退，直接报错。
   */
  binaryKill(
    serial: string,
    pid: number,
    root = false,
    expectedName?: string,
  ): Promise<void> {
    return invokeCommand<void>("device_binary_kill", {
      serial,
      pid,
      root,
      expectedName: expectedName || undefined,
    });
  },
  /** 查托管进程监听端口（/proc/<pid>/fd → /proc/net/tcp(6)） */
  binaryPorts(serial: string, pid: number, root = false): Promise<ListenPort[]> {
    return invokeCommand<ListenPort[]>("device_binary_ports", { serial, pid, root });
  },
  /** 进程端口互查：PID→端口（任意进程） */
  procPorts(serial: string, pid: number, root = false): Promise<ListenPort[]> {
    return invokeCommand<ListenPort[]>("device_proc_ports", { serial, pid, root });
  },
  /** 进程端口互查：端口→PID（LISTEN inode → /proc fd 持有者） */
  procByPort(serial: string, port: number, root = false): Promise<PortHolder[]> {
    return invokeCommand<PortHolder[]>("device_proc_by_port", { serial, port, root });
  },
  /** 以下长操作返回 task_id，输出走 task:// 事件流 */
  shell(serial: string, command: string): Promise<string> {
    return invokeCommand<string>("device_shell", { args: { serial, command } });
  },
  install(serial: string, apkPath: string): Promise<string> {
    return invokeCommand<string>("device_install", { args: { serial, apkPath } });
  },
  uninstall(serial: string, pkg: string): Promise<string> {
    return invokeCommand<string>("device_uninstall", { args: { serial, package: pkg } });
  },
  launch(serial: string, pkg: string): Promise<string> {
    return invokeCommand<string>("device_launch", { args: { serial, package: pkg } });
  },
  forceStop(serial: string, pkg: string): Promise<string> {
    return invokeCommand<string>("device_force_stop", { args: { serial, package: pkg } });
  },
  push(serial: string, local: string, remote: string): Promise<string> {
    return invokeCommand<string>("device_push", { args: { serial, local, remote } });
  },
  pull(serial: string, remote: string, local: string): Promise<string> {
    return invokeCommand<string>("device_pull", { args: { serial, remote, local } });
  },
  logcat(serial: string, filter?: string): Promise<string> {
    return invokeCommand<string>("device_logcat", {
      args: { serial, filter: filter ?? null },
    });
  },
  /** 设备热插拔事件（后端 watch 线程 diff 后推送） */
  onChanged(handler: (p: DeviceChangedPayload) => void) {
    return listenEvent<DeviceChangedPayload>("device://changed", handler);
  },
  /** so 替换预览：按 ABI 查询包安装 lib 目录（只读） */
  pkgLibDir(serial: string, pkg: string, abi: "arm64" | "arm"): Promise<string> {
    return invokeCommand<string>("device_pkg_lib_dir", { args: { serial, pkg, abi } });
  },
  /** so 替换：push → su -c cat 写回 → 清理；返回写入的目标路径 */
  soReplace(serial: string, localPath: string, pkg: string, abi: "arm64" | "arm"): Promise<string> {
    return invokeCommand<string>("device_so_replace", {
      args: { serial, localPath, pkg, abi },
    });
  },
  /** 任务状态事件（复用 task 协议） */
  onTaskStatus(handler: (p: TaskStatusPayload) => void) {
    return listenEvent<TaskStatusPayload>("task://status", handler);
  },
};
