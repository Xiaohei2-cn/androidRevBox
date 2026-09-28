use agent_protocol::{CapabilityInfo, HealthResult, PermissionInfo, ProviderInfo};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionState {
    Disconnected,
    AdbOnline,
    Installing,
    Starting,
    Handshaking,
    Ready,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionStatus {
    pub serial: String,
    pub state: AgentSessionState,
    pub agent_version: Option<String>,
    pub protocol_version: Option<u32>,
    pub permissions: Option<PermissionInfo>,
    pub providers: Vec<ProviderInfo>,
    pub capabilities: Vec<CapabilityInfo>,
    pub local_port: Option<u16>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiagnostics {
    pub status: AgentSessionStatus,
    pub health: Option<HealthResult>,
    pub health_error: Option<String>,
    pub routes: Vec<AgentRouteDiagnostics>,
    /// 最近一次自动连接探测的结论（AR12.5）。`None` = 这台设备还没探过（刚插上、
    /// 或自动连接被关着）。设备页那句「现在怎么了」读的就是它。
    pub auto_probe: Option<AgentProbe>,
    /// 本次运行里**真的走过** Legacy ADB 回退的累计次数（按能力+原因分开计）。
    ///
    /// 为什么专门加这个：AR12 要删回退腿，而"能不能删"不该靠"我印象里现在都走 Agent"
    /// 回答。`routes` 只保留每个能力最近一次决策、看不出频率；这里是累计计数，
    /// 让删除决定变成可观察的事实：计数为 0 的能力才可以安全砍掉。
    pub legacy_fallbacks: Vec<LegacyFallbackTotal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFallbackTotal {
    pub method: String,
    pub reason: String,
    pub count: u64,
    /// 登记的删除条件（来自 Legacy 能力表）；没登记为 None
    pub removal_stage: Option<String>,
}

/// 自动连接（AR12.5 / D086）：只读探测得出的「这台设备的 Agent 现在缺哪一步」。
///
/// 这个枚举是**给用户看的结论**，不是内部状态机：它的每一条都必须能翻译成界面上一句
/// 「所以现在怎么了」。加新条目时同步加 i18n 文案，不要把它变成第二个 `routes`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProbeDecision {
    /// 桌面已有可用会话——什么都不用做
    InSession,
    /// 产物已在设备上、版本一致、进程没在跑：只差「起进程 + 握手」
    IdleArtifactCurrent,
    /// 产物在设备上但和本次要用的不是同一份：需要覆盖安装（保留 rollback）
    StaleArtifact,
    /// 设备上还没有产物：需要推送并落地
    NotInstalled,
    /// 设备上有一个 Agent 在跑，但不是本进程连的。**不擅自重启**（D063：
    /// 重启会连带打掉托管进程与 frida-server 的账），由用户显式接管
    RunningElsewhere,
    /// 桌面侧找不到可用产物（开发态未构建、资源未打包）：不动设备
    ArtifactMissing,
    /// 设备 ABI 不是当前支持的 arm64-v8a（AR12.2 未收口）：不动设备
    UnsupportedAbi,
    /// 设备不在线/未授权 adb：不动设备
    DeviceOffline,
    /// 探测本身失败（adb 报错、脚本读不出来）：不动设备，照实说
    ProbeFailed,
}

/// 界面据探测结论要给用户的那一个动作。前端不自己推导——推导规则就是产品口径，
/// 必须只有一处（Rust 侧），否则网页和后端会对「该不该弹这个按钮」各说一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAutoAction {
    /// 没什么要用户决定的
    None,
    /// 需要用户点一次「装/起在本设备」——点完写进授权表，之后同 serial 全自动
    AskConsent,
    /// 已授权：自动连接会直接做，界面只报进度
    ConnectingAllowed,
    /// 有 Agent 在跑但不归本进程管：给「重启并接管」，并说清代价
    ExplicitTakeover,
    /// 装不了/不该装（产物缺失、ABI、设备不在线）：给原因，不给按钮
    Blocked,
}

/// 一次只读探测的结果（AR12.5）。camelCase 出前端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProbe {
    pub serial: String,
    pub decision: AgentProbeDecision,
    pub action: AgentAutoAction,
    /// 设备自报 ABI（`ro.product.cpu.abi`）；读不到为 None
    pub device_abi: Option<String>,
    /// 设备上那份产物的 sha256；没装为 None
    pub installed_sha256: Option<String>,
    /// 本次要用的产物 sha256；桌面侧解析失败为 None
    pub expected_sha256: Option<String>,
    pub agent_running: bool,
    /// 这台设备在授权表里吗（决定 AskConsent 还是直接动手）
    pub consent_granted: bool,
    /// 总开关 `app.agent.auto_connect`
    pub auto_enabled: bool,
    /// 给人看的一句原因（错误详情 / 为什么不动设备）
    pub detail: Option<String>,
    pub probed_at: i64,
}

/// 一次自动连接的实际落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAutoOutcome {
    /// 本来就有会话，没动设备
    AlreadyConnected,
    /// 走了完整连接流程并成功（可能含 push/start，取决于探测结论与授权）
    Connected,
    /// 未授权：**没往设备写任何东西**，等用户点一次
    AwaitedConsent,
    /// 有 Agent 在跑但不归本进程管：**没杀它**（D063）
    DeferredTakeover,
    /// 前置不满足（产物缺失 / ABI / 设备不在线）：没动设备
    Skipped,
    /// 已经在跑一次（同一 serial 的并发触发被合并）
    InFlight,
    /// 连接过程失败
    Failed,
}

/// 自动连接的完整回执（事件 payload 与命令返回共用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAutoRun {
    pub serial: String,
    pub outcome: AgentAutoOutcome,
    pub probe: AgentProbe,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidBackendSource {
    Agent,
    LegacyAdb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRouteDiagnostics {
    pub serial: String,
    pub method: String,
    pub backend: AndroidBackendSource,
    pub fallback_reason: Option<String>,
    pub agent_version: Option<String>,
    pub protocol_version: Option<u32>,
    pub recorded_at: i64,
}

impl AgentSessionStatus {
    pub fn disconnected(serial: impl Into<String>) -> Self {
        Self {
            serial: serial.into(),
            state: AgentSessionState::Disconnected,
            agent_version: None,
            protocol_version: None,
            permissions: None,
            providers: Vec::new(),
            capabilities: Vec::new(),
            local_port: None,
            last_error: None,
        }
    }

    pub(crate) fn transition(&mut self, state: AgentSessionState) {
        self.state = state;
        self.last_error = None;
    }

    pub(crate) fn fail(&mut self, state: AgentSessionState, error: impl Into<String>) {
        self.state = state;
        self.local_port = None;
        self.last_error = Some(error.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_state_uses_stable_case_and_hides_no_error_context() {
        let mut status = AgentSessionStatus::disconnected("serial-1");
        status.fail(AgentSessionState::Incompatible, "protocol mismatch");
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["state"], "incompatible");
        assert_eq!(value["serial"], "serial-1");
        assert_eq!(value["lastError"], "protocol mismatch");
    }
}
