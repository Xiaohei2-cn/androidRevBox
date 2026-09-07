//! 全项目统一错误类型：库层用 thiserror 保持稳定，应用层上下文用 anyhow。
//! 跨 IPC 边界序列化为结构化对象 { code, message }，约定见 docs/ipc-conventions.md。

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("数据库错误: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("内部错误: {0}")]
    Internal(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl CoreError {
    /// 稳定错误码，前端按此分支处理（与 docs/ipc-conventions.md 的表一致）
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::Io(_) => "IO",
            CoreError::Serialization(_) => "SERIALIZATION",
            CoreError::Database(_) => "DATABASE",
            CoreError::Internal(_) => "INTERNAL",
        }
    }
}

/// 错误到前端的统一映射：{ code, message }，message 含中文可读上下文
impl Serialize for CoreError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("CoreError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
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
    fn error_serializes_to_code_and_message() {
        let err = CoreError::Internal("测试".to_string());
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["code"], "INTERNAL");
        assert_eq!(json["message"], "内部错误: 测试");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no file");
        let err: CoreError = io_err.into();
        assert_eq!(err.code(), "IO");
    }

    #[test]
    fn rusqlite_error_converts_via_from() {
        let err: CoreError = rusqlite::Error::QueryReturnedNoRows.into();
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "DATABASE");
    }
}
