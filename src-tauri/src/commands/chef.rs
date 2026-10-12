//! 算法中心命令层（CyberChef 移植）。
//!
//! ⚠️ `chef_bake` 走 async + spawn_blocking：配方里可能含 PBKDF2/bcrypt 这类
//! 故意慢的算法（迭代十万次是它的工作方式，不是 bug），同步命令会把主线程冻住。
//! 算法级的失败（密钥长度不对、不是 UTF-8）留在 `ChefBakeResult.steps[].error` 里，
//! 这一层只拒绝**结构性**错误（输入超限、输入格式不认识）。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::models::chef::{ChefBakeRequest, ChefBakeResult, ChefOperationDto};
use crate::services::chef::data::ChefError;

fn into_core(error: ChefError) -> CoreError {
    CoreError::InvalidInput(error.message)
}

/// spawn_blocking 桥接（与插件命令层同一写法）：调度失败也是错误，不能默默返回空
async fn join<T: Send + 'static>(
    handle: tauri::async_runtime::JoinHandle<CoreResult<T>>,
    what: &str,
) -> CoreResult<T> {
    handle
        .await
        .map_err(|e| CoreError::Internal(format!("{what}任务调度失败: {e}")))?
}

#[tauri::command]
pub async fn chef_operations(
    state: tauri::State<'_, AppState>,
) -> CoreResult<Vec<ChefOperationDto>> {
    let chef = state.chef.clone();
    join(
        tauri::async_runtime::spawn_blocking(move || Ok(chef.operations())),
        "算法清单",
    )
    .await
}

#[tauri::command]
pub async fn chef_bake(
    state: tauri::State<'_, AppState>,
    request: ChefBakeRequest,
) -> CoreResult<ChefBakeResult> {
    let chef = state.chef.clone();
    join(
        tauri::async_runtime::spawn_blocking(move || chef.bake(&request).map_err(into_core)),
        "配方执行",
    )
    .await
}
