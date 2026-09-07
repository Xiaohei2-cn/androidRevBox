//! 设备命令层（P3）：校验参数 → DeviceService。
//! 短查询直接返回结果；长操作（shell/install/logcat/push/pull）一律返回
//! task_id，输出走 task://* 事件流（PHASES §1.3 不阻塞 IPC）。

use serde::Deserialize;

use crate::AppState;
use crate::adapters::adb::{DeviceEntry, DeviceInfo, FileEntry};
use crate::core::error::{CoreError, CoreResult};
use crate::services::device_service::{AdbEnvironment, DeviceChangedPayload};

// ===== 环境 / 列表 =====

#[tauri::command]
pub async fn adb_environment(state: tauri::State<'_, AppState>) -> CoreResult<AdbEnvironment> {
    Ok(state.device.environment().await)
}

#[tauri::command]
pub async fn adb_set_path(
    state: tauri::State<'_, AppState>,
    path: String,
) -> CoreResult<AdbEnvironment> {
    let path = path.trim().to_string();
    if !path.is_empty() {
        let p = std::path::Path::new(&path);
        if !p.exists() {
            return Err(CoreError::Internal(format!("文件不存在: {path}")));
        }
    }
    state
        .config
        .set(crate::services::config_service::KEY_ADB_PATH, &path)?;
    Ok(state.device.reprobe().await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesArgs {
    /// 仅返回 state=="device" 的就绪设备
    #[serde(default)]
    pub ready_only: bool,
}

#[tauri::command]
pub async fn devices_list(
    state: tauri::State<'_, AppState>,
    args: Option<DevicesArgs>,
) -> CoreResult<Vec<DeviceEntry>> {
    let ready_only = args.map(|a| a.ready_only).unwrap_or(false);
    let mut devices = state.device.list_devices().await?;
    if ready_only {
        devices.retain(|d| d.is_ready());
    }
    Ok(devices)
}

#[tauri::command]
pub async fn devices_watch_now(
    state: tauri::State<'_, AppState>,
) -> CoreResult<Vec<DeviceChangedPayload>> {
    // 强制立即轮询一次并返回 diff（前端启动时主动拉基线；常驻 diff 走事件）
    state.device.poll_once_manual().await
}

#[tauri::command]
pub async fn device_info(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<DeviceInfo> {
    state.device.device_info(&serial).await
}

// ===== 短命令（capture 类）=====

#[tauri::command]
pub async fn device_ls(
    state: tauri::State<'_, AppState>,
    serial: String,
    path: String,
) -> CoreResult<Vec<FileEntry>> {
    state.device.list_files(&serial, &path).await
}

#[tauri::command]
pub async fn device_packages(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Vec<String>> {
    state.device.list_packages(&serial).await
}

// ===== 长操作：返回 task_id =====

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellArgs {
    pub serial: String,
    pub command: String,
}

#[tauri::command]
pub async fn device_shell(
    state: tauri::State<'_, AppState>,
    args: ShellArgs,
) -> CoreResult<String> {
    if args.command.trim().is_empty() {
        return Err(CoreError::Internal("命令不能为空".to_string()));
    }
    state.device.start_shell(&args.serial, &args.command).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallArgs {
    pub serial: String,
    /// 本机 APK 路径（P0 约束：选择文件在 P7 由原生 dialog 完成，本阶段直接传路径）
    pub apk_path: String,
}

#[tauri::command]
pub async fn device_install(
    state: tauri::State<'_, AppState>,
    args: InstallArgs,
) -> CoreResult<String> {
    if args.apk_path.trim().is_empty() || !std::path::Path::new(&args.apk_path).exists() {
        return Err(CoreError::Internal("APK 文件不存在".to_string()));
    }
    state
        .device
        .start_install(&args.serial, &args.apk_path)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallArgs {
    pub serial: String,
    pub package: String,
}

#[tauri::command]
pub async fn device_uninstall(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<String> {
    state
        .device
        .start_uninstall(&args.serial, &args.package)
        .await
}

#[tauri::command]
pub async fn device_launch(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<String> {
    state.device.start_launch(&args.serial, &args.package).await
}

#[tauri::command]
pub async fn device_force_stop(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<String> {
    state
        .device
        .start_force_stop(&args.serial, &args.package)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferArgs {
    pub serial: String,
    pub local: String,
    pub remote: String,
}

#[tauri::command]
pub async fn device_push(
    state: tauri::State<'_, AppState>,
    args: FileTransferArgs,
) -> CoreResult<String> {
    state
        .device
        .start_push(&args.serial, &args.local, &args.remote)
        .await
}

#[tauri::command]
pub async fn device_pull(
    state: tauri::State<'_, AppState>,
    args: FileTransferArgs,
) -> CoreResult<String> {
    state
        .device
        .start_pull(&args.serial, &args.remote, &args.local)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatArgs {
    pub serial: String,
    #[serde(default)]
    pub filter: Option<String>,
}

#[tauri::command]
pub async fn device_logcat(
    state: tauri::State<'_, AppState>,
    args: LogcatArgs,
) -> CoreResult<String> {
    state
        .device
        .start_logcat(&args.serial, args.filter.as_deref())
        .await
}
