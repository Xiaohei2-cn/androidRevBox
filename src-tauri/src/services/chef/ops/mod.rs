//! 算法实现按领域分文件；每组贡献一张 `OPS` 表，`GROUPS` 决定「算法清单」里的分组顺序。
//!
//! 分类 id 是稳定契约（前端词典用 `chef.category.<id>` 出名称）：新增算法请挂到已有
//! 分类，实在不属于任何一类再加新的分类 id，并同步 5 份词典。

use crate::services::chef::spec::OpEntry;

pub mod cipher;
pub mod compression;
pub mod digest;
pub mod encoding;
pub mod format;
pub mod kdf;
pub mod text;

pub type Op = OpEntry;

/// 清单顺序 = 这里的顺序（组内顺序即组内展示顺序）
pub static GROUPS: &[&[Op]] = &[
    encoding::OPS,
    digest::OPS,
    kdf::OPS,
    cipher::OPS,
    compression::OPS,
    format::OPS,
    text::OPS,
];
