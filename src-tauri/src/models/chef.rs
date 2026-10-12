//! 算法中心（CyberChef 移植）的 IPC DTO。
//!
//! 前端形状与这里逐字段对齐（camelCase）：算法清单 `ChefOperationDto`、
//! 一次配方执行 `ChefBakeRequest` → `ChefBakeResult`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 一个算法的参数控件描述
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefArgDto {
    pub name: String,
    /// text | bool | number | choice
    pub kind: String,
    /// 默认值（字符串形态：bool 是 "true"/"false"，number 是十进制）
    pub default: String,
    /// choice 的候选值；其余类型为空调
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefOperationDto {
    pub id: String,
    /// 分类 id（界面用 `chef.category.<id>` 出名字）
    pub category: String,
    pub args: Vec<ChefArgDto>,
}

/// 配方里的一步
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefStepInput {
    pub id: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefBakeRequest {
    pub input: String,
    /// utf8 | hex | base64 | base64url | decimal（缺省 utf8）
    #[serde(default)]
    pub input_encoding: Option<String>,
    #[serde(default)]
    pub recipe: Vec<ChefStepInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefStepResult {
    pub id: String,
    pub ok: bool,
    pub bytes: usize,
    pub utf8: bool,
    pub preview: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChefBakeResult {
    pub ok: bool,
    pub output: String,
    pub output_bytes: usize,
    pub output_utf8: bool,
    pub output_hex: String,
    pub output_base64: String,
    pub steps: Vec<ChefStepResult>,
    pub failed_at: Option<usize>,
    pub input_bytes: usize,
}
