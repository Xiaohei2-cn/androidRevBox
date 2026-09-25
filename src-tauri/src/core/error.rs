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

    /// 设备侧明确回答"这个东西不存在"。以前它跟真的内部故障挤在同一个变体里，
    /// 于是文件页上一个普通的"路径没了"显示成
    /// `内部错误: Agent returned not_found: lstat 失败 /storage/emulated/0/sdcard...`，
    /// 用户只能猜是不是程序坏了。
    #[error("设备上没有这个文件或目录：{0}")]
    NotFound(String),

    /// 设备当前的状态与这次操作冲突（端口已被占用、同名进程已在跑…）。
    ///
    /// 单独立一个变体，理由与 `NotFound` 一模一样：这类话以前挤在「内部错误」里，
    /// 用户看到 `内部错误: auth-server 启动后立即退出：… bind failed` 第一反应是
    /// 程序坏了去翻日志，而真相是"已经有一个 auth-server 在跑、端口是它的"——
    /// 该做的是先停掉那个进程或换个端口，跟故障没关系。
    #[error("与设备当前状态冲突：{0}")]
    Conflict(String),

    #[error("Agent 不可用: {0}")]
    AgentUnavailable(String),

    #[error("Agent 版本不兼容: {0}")]
    AgentIncompatible(String),

    #[error("Agent 传输断开: {0}")]
    AgentTransportLost(String),

    /// 用户的输入还没补齐：没选设备、目标没填、端点写错、脚本不在工作目录里……
    ///
    /// 立这个变体的理由和 `NotFound`/`Conflict` 一模一样：这些话以前都塞在「内部错误」里，
    /// 于是 `内部错误: 目标应用（包名或 pid）不能为空` 这种提示会把人引去翻日志、怀疑程序
    /// 坏了，而真正该做的只是把那个空着的框填上。**故障和"你还没填"必须分得开。**
    #[error("还差一步：{0}")]
    InvalidInput(String),
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
            CoreError::NotFound(_) => "NOT_FOUND",
            CoreError::Conflict(_) => "CONFLICT",
            CoreError::AgentUnavailable(_) => "AGENT_UNAVAILABLE",
            CoreError::AgentIncompatible(_) => "AGENT_INCOMPATIBLE",
            CoreError::AgentTransportLost(_) => "AGENT_TRANSPORT_LOST",
            CoreError::InvalidInput(_) => "INVALID_INPUT",
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
    fn missing_input_is_not_reported_as_an_internal_failure() {
        // 用户在界面上少填一项，看到的必须是"还差一步"，不能是"内部错误"——
        // 后者会让人以为程序坏了去翻日志（这句提示真机出现过：目标应用为空）
        let err = CoreError::InvalidInput("目标应用要填包名或 pid".into());
        assert_eq!(err.code(), "INVALID_INPUT");
        assert_eq!(err.to_string(), "还差一步：目标应用要填包名或 pid");
        assert!(!err.to_string().contains("内部错误"));
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
