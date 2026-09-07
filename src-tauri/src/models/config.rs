//! 配置相关 DTO。

use serde::Serialize;

/// app_settings 表行的对外形状（值统一字符串，语义由 ConfigService 校验）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingDto {
    pub key: String,
    pub value: String,
}

impl From<(String, String)> for AppSettingDto {
    fn from((key, value): (String, String)) -> Self {
        Self { key, value }
    }
}
