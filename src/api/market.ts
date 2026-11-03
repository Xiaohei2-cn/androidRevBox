import { invokeCommand } from "./client";

/**
 * 工具市场的一项。
 *
 * 字段全部来自后端**内置目录**（代码里的常量，不是网络清单）：界面显示的大小与 SHA-256
 * 就是要装的那个文件的真实属性，安装后可以与设备侧复核对齐。
 */
export interface MarketTool {
  id: string;
  name: string;
  /** 一句话副标题（左侧气泡卡片上显示） */
  tagline: string;
  description: string;
  /** 上游仓库地址（官方地址） */
  homepage: string;
  license: string;
  version: string;
  downloadUrl: string;
  sizeBytes: number;
  sha256: string;
  deviceFileName: string;
  deviceAbi: string[];
  usage: string[];
  /**
   * 版本从哪来：`""`（或缺失）= 目录里登记的固定版本；
   * `"github-releases"` = 实时查上游 release（目前只有 frida-server）。
   * 动态条目的 version / downloadUrl / sizeBytes / sha256 都是空的，
   * 界面要等用户选好版本再走 `installFrida`。
   */
  versionSource?: "" | "github-releases" | null;
}

/** frida 官方 tags 里的一个版本（16.x 及以后）。资产信息在点安装时才按版本解析。 */
export interface FridaVersion {
  version: string;
}

export interface ToolInstallReport {
  id: string;
  version: string;
  serial: string;
  /** 设备侧绝对路径，可直接复制到命令行 */
  devicePath: string;
  bytes: number;
  sha256: string;
  /** 设备上 `stat -c %s` 复核出的字节数；`null` = 复核没做成，不等于没装上 */
  verifiedBytes: number | null;
  executable: boolean;
}

export const marketApi = {
  /** 内置工具目录（不联网） */
  tools(): Promise<MarketTool[]> {
    return invokeCommand<MarketTool[]>("market_tools");
  },
  /** 一键安装：下载 → 校验 → 推送 /data/local/tmp → chmod +x → 回读复核 */
  install(serial: string, id: string): Promise<ToolInstallReport> {
    return invokeCommand<ToolInstallReport>("market_install_tool", { serial, id });
  },
  /** 设备主 ABI；`null` = 读不到（未知），不等于不兼容 */
  deviceAbi(serial: string): Promise<string | null> {
    return invokeCommand<string | null>("market_device_abi", { serial });
  },
  /**
   * frida-server 的版本列表（实时取自 frida 官方 tags，16.x 及以后，从新到旧）。
   * 后端有 10 分钟内存缓存；`refresh = true` 强制重取（界面上的「重新获取」）。
   */
  fridaReleases(refresh = false): Promise<FridaVersion[]> {
    return invokeCommand<FridaVersion[]>("market_frida_releases", { refresh });
  },
  /**
   * 安装指定版本的 frida-server：解析资产 → 下载并核对 GitHub 登记的摘要
   * → 本机解压 .xz → 推送 `/data/local/tmp/frida-server` → chmod → 复核。
   */
  installFrida(serial: string, version: string): Promise<ToolInstallReport> {
    return invokeCommand<ToolInstallReport>("market_install_frida", { serial, version });
  },
};

/** 字节数的可读写法（与 frida 脚本列表同一套刻度） */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
