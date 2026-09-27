//! 翻译接口配置与调用（UI-6 第二层的可选环节）。
//!
//! 界面拿不到 API key 本体：`ai_config_get` 只回后 4 位，
//! key 的读写都留在桌面本地库里（见 `services::ai_service` 的三条纪律）。

use serde::Deserialize;

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::ai_service::{AiConfigView, TranslateOutcome};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigArgs {
    pub base_url: String,
    pub model: String,
    pub enabled: bool,
    /// `None` = 保持现有的 key 不动；`Some("")` = 清掉
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTranslateArgs {
    pub text: String,
    /// 目标语言名（界面语言，例如"中文"）
    pub target_lang: String,
    /// 起始语言；留空由接口自己判断（帮助文本几乎都是英文）
    pub source_lang: Option<String>,
}

#[tauri::command]
pub async fn ai_config_get(state: tauri::State<'_, AppState>) -> CoreResult<AiConfigView> {
    Ok(state.ai.view())
}

#[tauri::command]
pub async fn ai_config_set(
    state: tauri::State<'_, AppState>,
    args: AiConfigArgs,
) -> CoreResult<AiConfigView> {
    state.ai.save(
        &args.base_url,
        &args.model,
        args.enabled,
        args.api_key.as_deref(),
    )
}

/// 把探测到的 help 文本翻成界面语言。
///
/// 这条**只由用户点按钮触发**，且只发往用户自己填的地址：待译文本会离开本机，
/// 界面上必须把这件事说在前面（翻译开关旁边就写着"原文会发送到你配置的接口"）。
#[tauri::command]
pub async fn ai_translate(
    state: tauri::State<'_, AppState>,
    args: AiTranslateArgs,
) -> CoreResult<TranslateOutcome> {
    state
        .ai
        .translate(&args.text, &args.target_lang, args.source_lang.as_deref())
        .await
}
