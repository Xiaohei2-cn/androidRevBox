import { invokeCommand } from "./client";

export type ZygiskScope = "all" | "user" | "system";
export type ZygiskLabelSource = "framework" | "manifest" | "package_name";

export interface ZygiskAppItem {
  packageName: string;
  label: string;
  versionName: string;
  versionCode: number | null;
  labelSource: ZygiskLabelSource;
  requestedLocale: string;
  resolvedLocale: string | null;
  fallbackReason: string | null;
  uid: number | null;
  isSystem: boolean;
  enabled: boolean;
}

export interface ZygiskAppWarning {
  packageName: string | null;
  code: string;
  message: string;
}

export interface ZygiskAppList {
  items: ZygiskAppItem[];
  successCount: number;
  fallbackCount: number;
  warnings: ZygiskAppWarning[];
  deviceLocale: string | null;
}

export interface ZygiskStatus {
  /** not_installed | zygisk_disabled | installed_reboot_required | loaded | bridge_ready | incompatible | faulted */
  lifecycle: string;
  bridgeReady: boolean;
  rootAvailable: boolean;
  agentConnected: boolean;
  moduleId: string | null;
  moduleVersion: string | null;
  moduleVersionCode: number | null;
  zygiskImpl: string | null;
  deviceLocale: string | null;
  subProtocolVersion: number;
  probeLatencyMs: number | null;
  detail: string | null;
}

export interface ZygiskApkFile {
  packageName: string;
  name: string;
  size: number;
}

export interface ZygiskExportReport {
  packageName: string;
  files: ZygiskApkFile[];
  destination: string;
  bytes: number;
}

export const zygiskApi = {
  status(serial: string): Promise<ZygiskStatus> {
    return invokeCommand<ZygiskStatus>("zygisk_status", { serial });
  },
  list(
    serial: string,
    scope: ZygiskScope = "all",
    options?: { locale?: string | null; includeDisabled?: boolean },
  ): Promise<ZygiskAppList> {
    return invokeCommand<ZygiskAppList>("package_list_localized", {
      serial,
      scope,
      locale: options?.locale ?? null,
      includeDisabled: options?.includeDisabled ?? false,
    });
  },
  exportPackage(
    serial: string,
    packageName: string,
    destination: string,
  ): Promise<ZygiskExportReport> {
    return invokeCommand<ZygiskExportReport>("package_export_apk", {
      serial,
      packageName,
      destination,
    });
  },
};
