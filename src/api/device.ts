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
  packages(serial: string): Promise<string[]> {
    return invokeCommand<string[]>("device_packages", { serial });
  },
  /** 设备 wlan0 IPv4（adb -s <serial> shell ip addr show wlan0） */
  ip(serial: string): Promise<string | null> {
    return invokeCommand<string | null>("device_ip", { serial });
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
  /** 终止托管进程（kill -9 pid；root 启动的进程需 root=true） */
  binaryKill(serial: string, pid: number, root = false): Promise<void> {
    return invokeCommand<void>("device_binary_kill", { serial, pid, root });
  },
  /** 查托管进程监听端口（/proc/<pid>/fd → /proc/net/tcp(6)） */
  binaryPorts(serial: string, pid: number, root = false): Promise<ListenPort[]> {
    return invokeCommand<ListenPort[]>("device_binary_ports", { serial, pid, root });
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
  /** 任务状态事件（复用 task 协议） */
  onTaskStatus(handler: (p: TaskStatusPayload) => void) {
    return listenEvent<TaskStatusPayload>("task://status", handler);
  },
};
