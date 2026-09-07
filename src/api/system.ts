import type { SystemInfo } from "@/types/system";
import { invokeCommand } from "./client";

export const systemApi = {
  /** 后端连通性探测：返回应用与运行时信息 */
  ping(): Promise<SystemInfo> {
    return invokeCommand<SystemInfo>("system_ping");
  },
};
