import { invokeCommand } from "./client";

/** app_settings 行（Rust 侧 serde camelCase） */
export interface AppSettingDto {
  key: string;
  value: string;
}

/** 与 Rust commands/config.rs 一一对应的封装 */
export const configApi = {
  /** 拉取全部白名单配置键（前端启动时调用） */
  snapshot(): Promise<AppSettingDto[]> {
    return invokeCommand<AppSettingDto[]>("config_snapshot");
  },
  /** 写单个键；非法键/值会被后端以 {code,message} 拒绝 */
  set(key: string, value: string): Promise<null> {
    return invokeCommand<null>("config_set", { args: { key, value } });
  },
  /** 日志级别：联动后端运行时重载过滤器并持久化，返回生效级别 */
  setLogLevel(level: string): Promise<string> {
    return invokeCommand<string>("log_set_level", { level });
  },
};
