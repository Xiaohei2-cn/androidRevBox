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
}

export interface FileEntry {
  name: string;
  isDir: boolean;
  size: number;
  symlink?: string | null;
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
