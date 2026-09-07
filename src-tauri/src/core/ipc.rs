//! 前后端通信规范（P1 定稿，文档见 docs/ipc-conventions.md）：
//! - DTO：命令出入参统一 camelCase；
//! - 错误：命令一律返回 Result<T, CoreError>，CoreError 自身序列化为 { code, message }；
//! - 事件：统一 AppEvent 封装（event + timestamp + payload），事件名集中在 event_names 注册。
//!
//! 事件协议在 P1 冻结，业务构造点位于 P2–P4，允许暂时无构造者。
#![allow(dead_code)]

use serde::Serialize;

/// 事件名注册表：前端 listen 的字符串常量只在此定义一次。
/// 常量本身在 P2–P4 接入业务后才被构造，P1 先行冻结协议。
pub mod event_names {
    /// 任务实时输出（P2 启用）：payload = { taskId, stream, chunk, ts }
    pub const TASK_OUTPUT: &str = "task://output";
    /// 任务状态变更（P2 启用）：payload = { taskId, status, exitCode, finishedAt }
    pub const TASK_STATUS: &str = "task://status";
    /// 设备插拔（P3 启用）：payload = { serial, transport, present, lastSeen }
    pub const DEVICE_CHANGED: &str = "device://changed";
    /// 插件生命周期（P4 启用）：payload = { pluginId, phase, version? }
    pub const PLUGIN_CHANGED: &str = "plugin://changed";
}

/// 统一事件封装：event_name + timestamp(秒) + payload。
/// 长任务采用「命令返回 task_id + 事件流」，不让 IPC 请求长期阻塞。
/// P2 起由各 Service 构造并经 emit 推送，P1 先定协议。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEvent<P: Serialize> {
    pub event: &'static str,
    pub timestamp: u64,
    pub payload: P,
}

impl<P: Serialize> AppEvent<P> {
    pub fn new(event: &'static str, payload: P) -> Self {
        Self {
            event,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_event_serializes_with_envelope() {
        let evt = AppEvent::new(
            event_names::DEVICE_CHANGED,
            serde_json::json!({"serial": "emulator-5554"}),
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "device://changed");
        assert!(json["timestamp"].as_u64().unwrap() > 1_700_000_000);
        assert_eq!(json["payload"]["serial"], "emulator-5554");
    }
}
