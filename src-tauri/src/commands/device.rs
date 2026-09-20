//! 设备命令层（P3）：校验参数 → DeviceService。
//! 短查询直接返回结果；长操作（shell/install/logcat/push/pull）一律返回
//! task_id，输出走 task://* 事件流（PHASES §1.3 不阻塞 IPC）。

use agent_protocol::{
    FilesystemPreviewResult, FilesystemStatResult, HostedRunRecord, HostedStopResult,
    PackageUninstallResult, PackageWriteResult, ReplaceNativeLibraryResult,
};
use serde::Deserialize;

use crate::AppState;
use crate::adapters::adb::{
    DeviceEntry, DeviceInfo, FileEntry, HostedBinary, ListenPort, PortHolder,
};
use crate::core::error::{CoreError, CoreResult};
use crate::services::device_service::{AdbEnvironment, DeviceChangedPayload, ForwardRule};

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

/// 设备 wlan0 IP（adb -s <serial> shell ip addr show wlan0；用户指定命令）
#[tauri::command]
pub async fn device_ip(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Option<String>> {
    state.device.device_ip(&serial).await
}

// ===== 端口转发（P9 ADB 页；全部 -s 绑定设备，多设备互不串扰）=====

/// 建立转发：adb -s <serial> forward <local> <remote>
#[tauri::command]
pub async fn adb_forward_setup(
    state: tauri::State<'_, AppState>,
    serial: String,
    local: String,
    remote: String,
) -> CoreResult<ForwardRule> {
    let (s, l, r) = state.device.forward_setup(&serial, &local, &remote).await?;
    Ok(ForwardRule {
        serial: s,
        local: l,
        remote: r,
    })
}

/// 列出该设备当前全部转发规则（验证「是否生效」的数据源）
#[tauri::command]
pub async fn adb_forward_list(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Vec<ForwardRule>> {
    state.device.forward_list(&serial).await
}

/// 删除转发（local 缺省 = 删该设备全部规则）
#[tauri::command]
pub async fn adb_forward_remove(
    state: tauri::State<'_, AppState>,
    serial: String,
    local: Option<String>,
) -> CoreResult<()> {
    state.device.forward_remove(&serial, local.as_deref()).await
}

// ===== 二进制托管（/data/local/tmp；全部 -s 绑定设备）=====

/// 列出托管目录下的 ELF 文件（含执行权限）
#[tauri::command]
pub async fn device_binaries(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Vec<HostedBinary>> {
    state.device.hosted_binaries(&serial).await
}

/// 探测 su 可用性（Root 开关前置检查）
#[tauri::command]
pub async fn device_binary_su_check(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<bool> {
    state.device.su_available(&serial).await
}

/// 设备 Root 状态（设备信息卡横幅）：su -c id 可达 uid=0 即视为有 root
#[tauri::command]
pub async fn device_root_check(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<bool> {
    state.device.su_available(&serial).await
}

/// 赋予执行权限（chmod +x；root=true 走 su -c）
#[tauri::command]
pub async fn device_binary_chmod(
    state: tauri::State<'_, AppState>,
    serial: String,
    name: String,
    root: Option<bool>,
) -> CoreResult<()> {
    state
        .device
        .hosted_chmod(&serial, &name, root.unwrap_or(false))
        .await
}

/// 后台启动二进制，返回 pid（root=true 走 su -c）
#[tauri::command]
pub async fn device_binary_run(
    state: tauri::State<'_, AppState>,
    serial: String,
    name: String,
    root: Option<bool>,
) -> CoreResult<u32> {
    state
        .device
        .hosted_run(&serial, &name, root.unwrap_or(false))
        .await
}

/// 终止进程（AR6.3 写操作）。默认走 Agent "process.kill"（发信号前重读身份防 PID 复用）；
/// root=true 走 Legacy su -c；Agent 不可用时**不自动回退**，直接报错。
#[tauri::command]
pub async fn device_binary_kill(
    state: tauri::State<'_, AppState>,
    serial: String,
    pid: u32,
    root: Option<bool>,
    expected_name: Option<String>,
) -> CoreResult<()> {
    state
        .device
        .process_kill(&serial, pid, expected_name, root.unwrap_or(false))
        .await
}

/// 托管运行表（AR7.2，Agent only）：句柄 + pid + start time + 状态 + 退出码。
/// 页面刷新或 Desktop 重启后仍能显示「谁真的在跑」，不再依赖前端本地状态。
#[tauri::command]
pub async fn device_hosted_runs(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Vec<HostedRunRecord>> {
    state.device.hosted_runs(&serial).await
}

/// 按句柄停止托管进程（AR7.3 写操作，Agent only）。
/// Agent 发信号前会用落盘的 start time 复核身份；`expectedPid` 与记录不符
/// 说明列表已过期，会被 precondition_failed 拒止而不是照数字杀。
#[tauri::command]
pub async fn device_hosted_stop(
    state: tauri::State<'_, AppState>,
    serial: String,
    handle: String,
    expected_pid: Option<u32>,
) -> CoreResult<HostedStopResult> {
    state
        .device
        .hosted_stop(&serial, &handle, expected_pid)
        .await
}

/// 单路径元数据（AR7.1，Agent only）：type/mode/uid/gid/size/mtime/link target 结构化返回。
/// `follow_symlink=false` 是 lstat 语义（链接本身），true 时取解析后的目标。
#[tauri::command]
pub async fn device_file_stat(
    state: tauri::State<'_, AppState>,
    serial: String,
    path: String,
    follow_symlink: Option<bool>,
) -> CoreResult<FilesystemStatResult> {
    state
        .device
        .file_stat(&serial, &path, follow_symlink.unwrap_or(false))
        .await
}

/// 受限预览（AR7.1，Agent only）：文本按 utf8 返回，二进制按小写 hex；
/// `from_end=true` 是日志尾读语义。字节上限由 Agent 侧夹住（默认 64 KiB，最大 256 KiB）。
#[tauri::command]
pub async fn device_file_preview(
    state: tauri::State<'_, AppState>,
    serial: String,
    path: String,
    max_bytes: Option<u32>,
    from_end: Option<bool>,
) -> CoreResult<FilesystemPreviewResult> {
    state
        .device
        .file_preview(&serial, &path, max_bytes, from_end.unwrap_or(false))
        .await
}

/// 查托管进程监听端口（/proc/<pid>/fd socket inode → /proc/net/tcp(6)，
/// 十六进制还原后仅返回 LISTEN 态、去重升序）
#[tauri::command]
pub async fn device_binary_ports(
    state: tauri::State<'_, AppState>,
    serial: String,
    pid: u32,
    root: Option<bool>,
) -> CoreResult<Vec<ListenPort>> {
    state
        .device
        .hosted_ports(&serial, pid, root.unwrap_or(false))
        .await
}

/// PID→端口（任意进程）：/proc/<pid>/fd socket inode → /proc/net/tcp(6)
#[tauri::command]
pub async fn device_proc_ports(
    state: tauri::State<'_, AppState>,
    serial: String,
    pid: u32,
    root: Option<bool>,
) -> CoreResult<Vec<ListenPort>> {
    state
        .device
        .process_ports(&serial, pid, root.unwrap_or(false))
        .await
}

/// so 替换请求参数（camelCase 前端约定）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoReplaceArgs {
    pub serial: String,
    /// 主机侧修补好的 .so 完整路径
    pub local_path: String,
    /// 目标应用包名
    pub pkg: String,
    /// arm64(64位) | arm(32位)
    pub abi: String,
}

/// 查询包安装 lib 目录（按 ABI，so 替换预览，只读）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PkgLibDirArgs {
    pub serial: String,
    pub pkg: String,
    pub abi: String,
}

#[tauri::command]
pub async fn device_pkg_lib_dir(
    state: tauri::State<'_, AppState>,
    args: PkgLibDirArgs,
) -> CoreResult<String> {
    state
        .device
        .pkg_lib_dir(&args.serial, &args.pkg, &args.abi)
        .await
}

/// SO 替换（AR8.4）：Desktop push 到唯一暂存目录 → Agent 备份/原子安装/sha256 复核/
/// 失败自动回滚。返回**步骤链结果**（不再是一句路径字符串）：UI 要能看见到底哪一步做了、
/// 哪一步没做、有没有回滚。写操作不自动回退——Agent 不在线就是明确报错。
#[tauri::command]
pub async fn device_so_replace(
    state: tauri::State<'_, AppState>,
    args: SoReplaceArgs,
) -> CoreResult<ReplaceNativeLibraryResult> {
    let result = state
        .device
        .replace_native_library(
            &args.serial,
            std::path::Path::new(&args.local_path),
            &args.pkg,
            &args.abi,
        )
        .await?;
    Ok(result)
}

/// 端口→PID 反查（LISTEN 行 inode → 扫 /proc/[0-9]*/fd 找持有进程）
#[tauri::command]
pub async fn device_proc_by_port(
    state: tauri::State<'_, AppState>,
    serial: String,
    port: u16,
    root: Option<bool>,
) -> CoreResult<Vec<PortHolder>> {
    state
        .device
        .pids_by_port(&serial, port, root.unwrap_or(false))
        .await
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
    state
        .device
        .raw_shell_task(&args.serial, &args.command)
        .await
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
    /// `pm uninstall -k`：保留数据与缓存。默认 false，与迁移前 `adb uninstall` 语义一致
    #[serde(default)]
    pub keep_data: bool,
}

/// 卸载：Agent typed 结果（AR8.1 收尾）。返回步骤链与复核结论，不产任务卡。
#[tauri::command]
pub async fn device_uninstall(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<PackageUninstallResult> {
    state
        .device
        .uninstall(&args.serial, &args.package, args.keep_data)
        .await
}

/// 启动应用：Agent typed 结果（`verified` 表示真看到新 pid，`replayed` 表示幂等命中）。
#[tauri::command]
pub async fn device_launch(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<PackageWriteResult> {
    state.device.launch(&args.serial, &args.package).await
}

/// 强制停止：Agent typed 结果（`verified` 表示复核到 pid 消失）。
#[tauri::command]
pub async fn device_force_stop(
    state: tauri::State<'_, AppState>,
    args: UninstallArgs,
) -> CoreResult<PackageWriteResult> {
    state.device.force_stop(&args.serial, &args.package).await
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
        .raw_logcat_task(&args.serial, args.filter.as_deref())
        .await
}
