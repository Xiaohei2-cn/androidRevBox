import { invokeCommand } from "./client";

export interface ZygiskAppItem {
  packageName: string;
  label: string;
  versionName: string;
  versionCode: number;
}

export interface ZygiskApkFile {
  packageName: string;
  name: string;
  size: number;
}

export interface ZygiskApkManifestEntry {
  name: string;
  path: string;
  size: number;
}

export type ZygiskApkManifest = Record<string, ZygiskApkManifestEntry[]>;

export interface ZygiskExportReport {
  packageName: string;
  files: ZygiskApkFile[];
  destination: string;
  bytes: number;
}

export const zygiskApi = {
  list(serial: string): Promise<ZygiskAppItem[]> {
    return invokeCommand<ZygiskAppItem[]>("zygisk_applist", { serial });
  },
  manifest(serial: string): Promise<ZygiskApkManifest> {
    return invokeCommand<ZygiskApkManifest>("zygisk_apk_manifest", { serial });
  },
  exportPackage(
    serial: string,
    packageName: string,
    destination: string,
  ): Promise<ZygiskExportReport> {
    return invokeCommand<ZygiskExportReport>("zygisk_applist_export", {
      serial,
      packageName,
      destination,
    });
  },
};
