//! 算法注册表：一个算法 = 一条 `OpEntry`（id / 分类 / 参数表 / 执行函数）。
//!
//! 为什么清单在 Rust 里而不是前端：
//! 前端只拿到「有哪些算法、每个算法要什么参数」，算法实现和参数语义必须同源，
//! 否则会出现界面上有 AES-CBC、后端只认 CBC 这种两侧各自演化的漂移。
//! 展示文案（名称/说明）留在前端词典里按 `chef.op.<id>.name` 取——
//! 语言是界面关注点，不是算法关注点，且 `pnpm i18n:check` 只在那里能校验。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::models::chef::{ChefArgDto, ChefOperationDto};

use super::data::{Args, ChefData, ChefError, ChefResult};
use super::ops;

/// 参数控件类型（前端据此渲染输入框 / 复选框 / 数字框 / 下拉框）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    Text {
        default: &'static str,
    },
    Bool {
        default: bool,
    },
    Number {
        default: u64,
    },
    Choice {
        default: &'static str,
        options: &'static [&'static str],
    },
}

#[derive(Debug, Clone, Copy)]
pub struct ArgSpec {
    pub name: &'static str,
    pub kind: ArgKind,
}

/// 一条算法
#[derive(Debug, Clone, Copy)]
pub struct OpEntry {
    pub id: &'static str,
    pub category: &'static str,
    pub args: &'static [ArgSpec],
    pub run: fn(&ChefData, &Args) -> ChefResult<ChefData>,
}

// const fn 构造器：让分类里的算法表能写成 `const`，不必在运行期堆 Vec
pub const fn text(name: &'static str, default: &'static str) -> ArgSpec {
    ArgSpec {
        name,
        kind: ArgKind::Text { default },
    }
}

pub const fn bool_arg(name: &'static str, default: bool) -> ArgSpec {
    ArgSpec {
        name,
        kind: ArgKind::Bool { default },
    }
}

pub const fn number(name: &'static str, default: u64) -> ArgSpec {
    ArgSpec {
        name,
        kind: ArgKind::Number { default },
    }
}

pub const fn choice(
    name: &'static str,
    default: &'static str,
    options: &'static [&'static str],
) -> ArgSpec {
    ArgSpec {
        name,
        kind: ArgKind::Choice { default, options },
    }
}

pub const fn op(
    id: &'static str,
    category: &'static str,
    args: &'static [ArgSpec],
    run: fn(&ChefData, &Args) -> ChefResult<ChefData>,
) -> OpEntry {
    OpEntry {
        id,
        category,
        args,
        run,
    }
}

/// 全量注册表（顺序即「算法清单」里的默认展示顺序）
pub static REGISTRY: LazyLock<Vec<OpEntry>> = LazyLock::new(|| {
    let mut all: Vec<OpEntry> = Vec::new();
    for group in ops::GROUPS {
        all.extend_from_slice(group);
    }
    all
});

pub fn find(id: &str) -> Option<&'static OpEntry> {
    REGISTRY.iter().find(|entry| entry.id == id)
}

/// 给前端的算法清单 DTO（展示文案在前端词典，这里只出结构与默认值）
pub fn operations() -> Vec<ChefOperationDto> {
    REGISTRY
        .iter()
        .map(|entry| ChefOperationDto {
            id: entry.id.to_string(),
            category: entry.category.to_string(),
            args: entry
                .args
                .iter()
                .map(|arg| match arg.kind {
                    ArgKind::Text { default } => ChefArgDto {
                        name: arg.name.to_string(),
                        kind: "text".into(),
                        default: default.to_string(),
                        options: vec![],
                    },
                    ArgKind::Bool { default } => ChefArgDto {
                        name: arg.name.to_string(),
                        kind: "bool".into(),
                        default: default.to_string(),
                        options: vec![],
                    },
                    ArgKind::Number { default } => ChefArgDto {
                        name: arg.name.to_string(),
                        kind: "number".into(),
                        default: default.to_string(),
                        options: vec![],
                    },
                    ArgKind::Choice { default, options } => ChefArgDto {
                        name: arg.name.to_string(),
                        kind: "choice".into(),
                        default: default.to_string(),
                        options: options.iter().map(|o| o.to_string()).collect(),
                    },
                })
                .collect(),
        })
        .collect()
}

/// 参数缺省时按注册表补齐：前端只交用户改过的项，未改的项由这里给默认值，
/// 保证「界面上没显示的默认」与「算法实际用的默认」是同一个来源。
/// 用户**清空**的输入框按空串交给算法（那是明确意图，不是没填）：
/// 「分隔符留空」这类需求必须能表达出来。
pub fn fill_defaults(entry: &OpEntry, given: &BTreeMap<String, String>) -> Args {
    let mut out = Args::new();
    for arg in entry.args {
        let default: String = match arg.kind {
            ArgKind::Text { default } => default.to_string(),
            ArgKind::Bool { default } => default.to_string(),
            ArgKind::Number { default } => default.to_string(),
            ArgKind::Choice { default, .. } => default.to_string(),
        };
        let value = match given.get(arg.name) {
            Some(provided) => provided.clone(),
            None => default,
        };
        out.insert(arg.name, value);
    }
    out
}

pub fn unknown_operation(id: &str) -> ChefError {
    ChefError::new(format!(
        "没有这个算法：{id}（清单可能在两侧不同步，重新加载页面试试）"
    ))
}
