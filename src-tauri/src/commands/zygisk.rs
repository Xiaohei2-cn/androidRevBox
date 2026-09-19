use std::path::PathBuf;

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::zygisk_applist::{
    LocalizedScope, ZygiskAppList, ZygiskApplistService, ZygiskExportReport, ZygiskStatus,
};

/// Zygisk 模块生命周期诊断（经 Agent ZygiskProvider，不直连模块）。
#[tauri::command]
pub async fn zygisk_status(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<ZygiskStatus> {
    state.zygisk_applist.status(&serial).await
}

/// `package.list_localized`：Framework 解析后的应用显示名批量清单。
/// `locale` 省略时使用设备默认 locale（手机语言为中文即返回中文清单）。
#[tauri::command]
pub async fn package_list_localized(
    state: tauri::State<'_, AppState>,
    serial: String,
    locale: Option<String>,
    scope: Option<LocalizedScope>,
    include_disabled: Option<bool>,
) -> CoreResult<ZygiskAppList> {
    state
        .zygisk_applist
        .list(
            &serial,
            locale,
            scope.unwrap_or(LocalizedScope::All),
            include_disabled.unwrap_or(false),
        )
        .await
}

/// 导出 base + split APK：Agent 设备侧暂存后经 ADB pull 取回，暂存随后回收。
#[tauri::command]
pub async fn package_export_apk(
    state: tauri::State<'_, AppState>,
    serial: String,
    package_name: String,
    destination: String,
) -> CoreResult<ZygiskExportReport> {
    state
        .zygisk_applist
        .export_package(&serial, &package_name, &PathBuf::from(destination))
        .await
}

#[allow(dead_code)]
fn _service_type_is_send_sync(_: &ZygiskApplistService) {}
