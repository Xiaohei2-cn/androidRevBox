use std::path::PathBuf;

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::zygisk_applist::{
    ZygiskApkManifestEntry, ZygiskAppItem, ZygiskApplistService, ZygiskExportReport,
};

/// 通过独立 Zygisk 模块读取 Framework 解析后的应用显示名。
#[tauri::command]
pub async fn zygisk_applist(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Vec<ZygiskAppItem>> {
    state.zygisk_applist.list(&serial).await
}

/// 通过模块读取每包 base + split APK 文件清单（E 命令，不含文件体）。
#[tauri::command]
pub async fn zygisk_apk_manifest(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<std::collections::BTreeMap<String, Vec<ZygiskApkManifestEntry>>> {
    state.zygisk_applist.apk_manifest(&serial).await
}

/// 通过模块流式导出 base.apk 与 split APK 到用户选择的本地目录。
#[tauri::command]
pub async fn zygisk_applist_export(
    state: tauri::State<'_, AppState>,
    serial: String,
    package_name: String,
    destination: String,
) -> CoreResult<ZygiskExportReport> {
    let destination = PathBuf::from(destination);
    state
        .zygisk_applist
        .export_package(&serial, &package_name, &destination)
        .await
}

#[allow(dead_code)]
fn _service_type_is_send_sync(_: &ZygiskApplistService) {}
