/** system_ping 命令返回的运行时信息（Rust 侧 serde camelCase） */
export interface SystemInfo {
  appVersion: string;
  tauriVersion: string;
  os: string;
  arch: string;
}
