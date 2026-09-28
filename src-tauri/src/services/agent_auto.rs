//! 自动连接（AR12.5 / D086）：设备一上线就把 Agent 接起来，但**动手之前先问一次**。
//!
//! 这一层为什么存在、为什么长成这个样子：
//!
//! 用户的反馈是两件事。① 设备页那行「ADB 回退次数」是 AR12 用来决定"能不能删回退腿"
//! 的内部证据，用户读不懂也不该读懂——他要的是「我现在这个操作到底能不能成」。
//! ② Agent 不在时一堆后续功能直接没腿，却要人先知道"有个 Agent 要先装"。
//! 所以这里把"发现 Agent 不在"从人的动作改成程序的动作，把"要不要往设备写东西"
//! 从程序的暗动作改成人的一次明动作。
//!
//! 三条边界（都对应已有的决策记录，不是这里新发明的顾虑）：
//! - **探测永远只读**：`get-state` / `getprop abi` / `sha256sum` / `pidof`。
//! - **写入与起进程受授权表管**（D028：静默装 Agent 会在用户没打算装的时候往设备
//!   写二进制）。授权是**每台设备一次**、持久化的，于是"自动"从第二次插线起就全自动
//!   ——既满足「别让我点」，也没把「不许悄悄写设备」废掉。
//! - **绝不重启不归本进程管的 Agent**（D063：重启会连带打掉托管进程与 frida-server
//!   的账）。这种情况只报"有个 Agent 在跑，但不是本程序连的"，接管必须用户明确点。
//!
//! 关掉 `app.agent.auto_connect`：watch 那条路一次 adb 调用都不发，界面回到
//! 「人点『安装并连接』」的老手感。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex as AsyncMutex;

use crate::adapters::agent_bootstrap::{AgentBootstrapError, DeviceAbi};
use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{
    AgentAutoAction, AgentAutoOutcome, AgentAutoRun, AgentProbe, AgentProbeDecision,
    AgentSessionState, AgentSessionStatus,
};
use crate::services::agent_artifact::AgentArtifactError;
use crate::services::agent_manager::AgentManager;
use crate::services::config_service::{
    ConfigService, KEY_AGENT_AUTO_CONNECT, KEY_AGENT_CONSENT_SERIALS,
};

/// 探测在「碰设备」之前/之中撞到的硬前置。它决定界面显示的是原因还是按钮。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeFailure {
    /// 桌面侧没有可用产物（没构建、没随包发布）
    ArtifactMissing(String),
    /// 设备 ABI 不是当前唯一支持的 arm64-v8a（AR12.2 未收口）
    UnsupportedAbi(String),
    /// 设备不在线 / adb 未授权
    DeviceOffline(String),
    /// 探测命令本身失败（adb 报错、输出读不出）
    ProbeFailed(String),
}

impl ProbeFailure {
    fn decision(&self) -> AgentProbeDecision {
        match self {
            Self::ArtifactMissing(_) => AgentProbeDecision::ArtifactMissing,
            Self::UnsupportedAbi(_) => AgentProbeDecision::UnsupportedAbi,
            Self::DeviceOffline(_) => AgentProbeDecision::DeviceOffline,
            Self::ProbeFailed(_) => AgentProbeDecision::ProbeFailed,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::ArtifactMissing(text)
            | Self::UnsupportedAbi(text)
            | Self::DeviceOffline(text)
            | Self::ProbeFailed(text) => text,
        }
    }
}

/// `decide` 的输入。把「事实」和「结论」分开写，是为了让结论这一半能脱离 adb 单测——
/// 否则"什么时候允许写设备"这条最要紧的规则只能靠真机腿证明，而真机腿不会每次跑。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeFacts {
    session_ready: bool,
    auto_enabled: bool,
    consent_granted: bool,
    device_abi: Option<String>,
    expected_sha256: Option<String>,
    installed_sha256: Option<String>,
    agent_running: bool,
    failure: Option<ProbeFailure>,
}

/// 纯函数：事实 -> （结论，界面该给的那一个动作，一句原因）。
///
/// 顺序即优先级，头两条不能倒：
/// - 已经有会话就先报"没事"，否则设备页每 10s 轮询会对着一条早就连好的会话反复弹按钮；
/// - 硬前置先于"有没有在跑"——产物都读不到时，"要不要接管"这个问题本身没有意义。
fn decide(facts: &ProbeFacts) -> (AgentProbeDecision, AgentAutoAction, Option<String>) {
    if facts.session_ready {
        return (AgentProbeDecision::InSession, AgentAutoAction::None, None);
    }
    if let Some(failure) = &facts.failure {
        return (
            failure.decision(),
            AgentAutoAction::Blocked,
            Some(failure.detail().to_owned()),
        );
    }
    let decision = if facts.agent_running {
        AgentProbeDecision::RunningElsewhere
    } else {
        match (&facts.installed_sha256, &facts.expected_sha256) {
            (Some(installed), Some(expected)) if installed == expected => {
                AgentProbeDecision::IdleArtifactCurrent
            }
            (Some(_), Some(_)) => AgentProbeDecision::StaleArtifact,
            (None, Some(_)) => AgentProbeDecision::NotInstalled,
            // 走到这里说明探测没失败，那这两项不该缺。真缺就照实报，不猜成"没装"——
            // 猜错一次等于白推一份产物、白做一次覆盖安装。
            _ => {
                return (
                    AgentProbeDecision::ProbeFailed,
                    AgentAutoAction::Blocked,
                    Some("探测结果不完整：拿不到本次要用的产物摘要，或设备上那份的摘要".into()),
                );
            }
        }
    };
    if !facts.auto_enabled {
        return (
            decision,
            AgentAutoAction::None,
            Some(
                "自动连接已关闭（设置 → 设备上线后自动连接 Agent），需要手动点「安装并连接」"
                    .into(),
            ),
        );
    }
    if decision == AgentProbeDecision::RunningElsewhere {
        // D063：不归本进程管的 Agent 不能顺手重启。
        return (
            decision,
            AgentAutoAction::ExplicitTakeover,
            Some(
                "设备上已有 Agent 在跑，但不是本程序连上的；重启会连带打掉托管进程与 frida-server"
                    .into(),
            ),
        );
    }
    let action = if facts.consent_granted {
        AgentAutoAction::ConnectingAllowed
    } else {
        AgentAutoAction::AskConsent
    };
    (decision, action, None)
}

pub struct AgentAutoService {
    agent: Arc<AgentManager>,
    config: Arc<ConfigService>,
    /// serial -> 最近一次探测结论。命令与事件都读这份缓存，不在前端轮询里重复碰设备。
    probes: Mutex<HashMap<String, AgentProbe>>,
    /// 同一台设备的并发触发只跑一次（watch 线程和用户手点会撞在一起）
    inflight: AsyncMutex<HashSet<String>>,
}

impl AgentAutoService {
    pub fn new(agent: Arc<AgentManager>, config: Arc<ConfigService>) -> Self {
        Self {
            agent,
            config,
            probes: Mutex::new(HashMap::new()),
            inflight: AsyncMutex::new(HashSet::new()),
        }
    }

    pub fn auto_enabled(&self) -> bool {
        self.config
            .get(KEY_AGENT_AUTO_CONNECT, "true")
            .map(|value| value == "true")
            .unwrap_or(true)
    }

    fn consent_list(&self) -> Vec<String> {
        self.config
            .get(KEY_AGENT_CONSENT_SERIALS, "")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn has_consent(&self, serial: &str) -> bool {
        let serial = serial.trim();
        !serial.is_empty() && self.consent_list().iter().any(|item| item == serial)
    }

    /// 记/撤一台设备的授权。写回的是规范化后的逗号串（空列表 = 空串）。
    ///
    /// 值校验交给 `ConfigService` 自己那张表，不在这里复制一份规则：两处规则一旦
    /// 分叉，就会出现"这里能写进去、那里读不出来"。
    pub fn set_consent(&self, serial: &str, granted: bool) -> CoreResult<()> {
        let serial = serial.trim();
        if serial.is_empty() {
            return Err(CoreError::Internal("设备序列号为空，不能记授权".into()));
        }
        let mut list = self.consent_list();
        list.retain(|item| item != serial);
        if granted {
            list.push(serial.to_owned());
        }
        self.config.set(KEY_AGENT_CONSENT_SERIALS, &list.join(","))
    }

    /// 缓存里的最近一次探测（无 IO）。`agent_diagnostics` 读的就是这个。
    pub fn cached_probe(&self, serial: &str) -> Option<AgentProbe> {
        self.probes
            .lock()
            .map(|guard| guard.get(serial).cloned())
            .unwrap_or_default()
    }

    /// **只读**探测：碰设备，但不写、不起、不杀任何东西。
    pub async fn probe(&self, serial: &str) -> AgentProbe {
        let status = self.agent.status(serial);
        let probe = build_probe(serial, &self.gather_facts(serial, &status).await);
        if let Ok(mut guard) = self.probes.lock() {
            guard.insert(serial.to_owned(), probe.clone());
        }
        probe
    }

    async fn gather_facts(&self, serial: &str, status: &AgentSessionStatus) -> ProbeFacts {
        let base = ProbeFacts {
            session_ready: matches!(
                status.state,
                AgentSessionState::Ready | AgentSessionState::Degraded
            ),
            auto_enabled: self.auto_enabled(),
            consent_granted: self.has_consent(serial),
            device_abi: None,
            expected_sha256: None,
            installed_sha256: None,
            agent_running: false,
            failure: None,
        };
        // 已经有会话就别再碰设备：结论是给界面读的，每秒问一次 adb 不叫探测，叫骚扰。
        if base.session_ready {
            return base;
        }
        let bootstrap = self.agent.bootstrap().clone();
        let artifact = match self.agent.artifacts().resolve(DeviceAbi::Arm64V8a) {
            Ok(artifact) => artifact,
            Err(error) => {
                return ProbeFacts {
                    failure: Some(ProbeFailure::ArtifactMissing(artifact_error_text(&error))),
                    ..base
                };
            }
        };
        if let Err(error) = bootstrap.ensure_online(serial).await {
            return ProbeFacts {
                failure: Some(bootstrap_failure(error)),
                ..base
            };
        }
        let abi = match bootstrap.device_abi(serial).await {
            Ok(abi) => abi,
            Err(error) => {
                return ProbeFacts {
                    failure: Some(bootstrap_failure(error)),
                    ..base
                };
            }
        };
        if abi != DeviceAbi::Arm64V8a {
            return ProbeFacts {
                device_abi: Some(abi.as_str().to_owned()),
                failure: Some(ProbeFailure::UnsupportedAbi(format!(
                    "本机 Agent 产物只提供 arm64-v8a，这台设备是 {}",
                    abi.as_str()
                ))),
                ..base
            };
        }
        let expected = Some(artifact.sha256);
        let installed = match bootstrap.installed_sha256(serial).await {
            Ok(installed) => installed,
            Err(error) => {
                return ProbeFacts {
                    device_abi: Some(abi.as_str().to_owned()),
                    expected_sha256: expected,
                    failure: Some(bootstrap_failure(error)),
                    ..base
                };
            }
        };
        let running = match bootstrap.is_running(serial).await {
            Ok(running) => running,
            Err(error) => {
                return ProbeFacts {
                    device_abi: Some(abi.as_str().to_owned()),
                    expected_sha256: expected,
                    installed_sha256: installed,
                    failure: Some(bootstrap_failure(error)),
                    ..base
                };
            }
        };
        ProbeFacts {
            device_abi: Some(abi.as_str().to_owned()),
            expected_sha256: expected,
            installed_sha256: installed,
            agent_running: running,
            ..base
        }
    }

    /// 设备上线时 watch 调的唯一入口，也是"连接"这件事**唯一**的实现处。
    ///
    /// 用户在设备页点「安装并连接 / 重启并接管」走的是 `commands::agent::agent_install`
    /// /`agent_restart`：先 `set_consent`（那一击就是授权），再直接 `connect_resolved`。
    /// 那里不经过这个方法，是为了让 `map_agent_error` 的 typed 错误（不兼容、传输断开）
    /// 原样回到界面——别在这里再开第二份"先授权再连"，两份实现迟早分叉。
    ///
    /// `AwaitedConsent` / `DeferredTakeover` / `Skipped` 三条支路**保证一个写入都没
    /// 发生**——每条都有单测钉住"零写入"，因为这句承诺是整个设计的全部意义。
    pub async fn auto_connect(&self, serial: &str) -> AgentAutoRun {
        if !self.auto_enabled() {
            // 关着就一次 adb 都不发：这样"关掉 = 回到老样子"是可验证的事实。
            return AgentAutoRun {
                serial: serial.to_owned(),
                outcome: AgentAutoOutcome::Skipped,
                probe: self.cached_probe(serial).unwrap_or_else(|| {
                    build_probe(serial, &offline_facts(false, self.has_consent(serial)))
                }),
                error: None,
            };
        }
        let probe = self.probe(serial).await;
        match probe.action {
            AgentAutoAction::AskConsent => AgentAutoRun {
                serial: probe.serial.clone(),
                outcome: AgentAutoOutcome::AwaitedConsent,
                probe,
                error: None,
            },
            AgentAutoAction::ExplicitTakeover => AgentAutoRun {
                serial: probe.serial.clone(),
                outcome: AgentAutoOutcome::DeferredTakeover,
                probe,
                error: None,
            },
            AgentAutoAction::ConnectingAllowed => self.connect_now(probe).await,
            // None（已在会话）/ Blocked（前置不满足）：都不该动手
            _ => AgentAutoRun {
                serial: probe.serial.clone(),
                outcome: if probe.decision == AgentProbeDecision::InSession {
                    AgentAutoOutcome::AlreadyConnected
                } else {
                    AgentAutoOutcome::Skipped
                },
                probe,
                error: None,
            },
        }
    }

    async fn connect_now(&self, mut probe: AgentProbe) -> AgentAutoRun {
        let serial = probe.serial.clone();
        {
            // 只把"占位"这一步锁住，绝不跨 await 持锁：连接要几十秒（推产物），
            // 锁跨过去就等于给 A 装机时把 B 的自动连接整个堵住——多设备直接不能用。
            let mut guard = self.inflight.lock().await;
            if guard.contains(&serial) {
                return AgentAutoRun {
                    serial,
                    outcome: AgentAutoOutcome::InFlight,
                    probe,
                    error: None,
                };
            }
            guard.insert(serial.clone());
        }
        probe.action = AgentAutoAction::ConnectingAllowed;
        let result = self.agent.connect_resolved(&serial).await;
        self.inflight.lock().await.remove(&serial);
        match result {
            Ok(status) => {
                tracing::info!(serial, state = ?status.state, "自动连接 Agent 成功");
                // 连上之后重探：此时会话已 ready，探测不再碰设备，只把结论刷成 InSession。
                let fresh = self.probe(&serial).await;
                AgentAutoRun {
                    serial,
                    outcome: AgentAutoOutcome::Connected,
                    probe: fresh,
                    error: None,
                }
            }
            Err(error) => {
                let detail = error.to_string();
                tracing::warn!(serial, error = %detail, "自动连接 Agent 失败");
                let fresh = self.probe(&serial).await;
                // 失败后重探可能因为会话被置回 disconnected 而得出"没装"之类更难看的答案，
                // 那种情况下保留失败前那份结论 + 错误原文，比刷成"看起来没事"诚实。
                AgentAutoRun {
                    serial,
                    outcome: AgentAutoOutcome::Failed,
                    probe: fresh,
                    error: Some(detail),
                }
            }
        }
    }
}

fn offline_facts(auto_enabled: bool, consent_granted: bool) -> ProbeFacts {
    ProbeFacts {
        session_ready: false,
        auto_enabled,
        consent_granted,
        device_abi: None,
        expected_sha256: None,
        installed_sha256: None,
        agent_running: false,
        failure: None,
    }
}

fn build_probe(serial: &str, facts: &ProbeFacts) -> AgentProbe {
    let (decision, action, detail) = decide(facts);
    AgentProbe {
        serial: serial.to_owned(),
        decision,
        action,
        device_abi: facts.device_abi.clone(),
        expected_sha256: facts.expected_sha256.clone(),
        installed_sha256: facts.installed_sha256.clone(),
        agent_running: facts.agent_running,
        consent_granted: facts.consent_granted,
        auto_enabled: facts.auto_enabled,
        detail,
        probed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    }
}

fn artifact_error_text(error: &AgentArtifactError) -> String {
    match error {
        AgentArtifactError::NotFound => {
            "本机没有可用的 Agent 产物（未构建，或未随安装包发布）".into()
        }
        other => other.to_string(),
    }
}

fn bootstrap_failure(error: AgentBootstrapError) -> ProbeFailure {
    match error {
        AgentBootstrapError::DeviceOffline { detail, .. } => ProbeFailure::DeviceOffline(detail),
        AgentBootstrapError::AdbUnavailable(detail) => ProbeFailure::DeviceOffline(detail),
        AgentBootstrapError::UnsupportedAbi { abi, .. } => {
            ProbeFailure::UnsupportedAbi(format!("设备 ABI 是 {abi}，当前只支持 arm64-v8a"))
        }
        other => ProbeFailure::ProbeFailed(other.to_string()),
    }
}

#[cfg(test)]
mod tests;
