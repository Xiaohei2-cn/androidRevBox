//! 全项目统一错误类型：库层用 thiserror 保持稳定，应用层上下文用 anyhow。

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serialization(#[from] serde_json::Error),

    // P0 骨架暂无构造点，P1 起由各 Service 使用
    #[allow(dead_code)]
    #[error("内部错误: {0}")]
    Internal(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

/// 跨 IPC 边界时错误统一序列化为字符串，前端拿到可读信息
impl Serialize for CoreError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_contains_context() {
        let err = CoreError::Internal("数据库未初始化".to_string());
        assert_eq!(err.to_string(), "内部错误: 数据库未初始化");
    }

    #[test]
    fn error_serializes_to_readable_string() {
        let err = CoreError::Internal("测试".to_string());
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, "\"内部错误: 测试\"");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no file");
        let err: CoreError = io_err.into();
        assert!(matches!(err, CoreError::Io(_)));
    }
}
