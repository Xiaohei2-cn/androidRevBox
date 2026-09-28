//! 自动连接的判断与边界。
//!
//! 这一组测试只钉一件事：**什么时候允许碰设备**。`decide` 是纯函数，所以"结论"能脱离
//! adb 全表跑一遍；"零写入"用会记录命令的假 runner 证明——只测结论的话，后来有人在
//! `AskConsent` 支路上加一句"顺手先起一下"，测试照样是绿的。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::core::error::CoreResult;
use crate::db::Db;
use crate::services::agent_artifact::AgentArtifactResolver;
use crate::services::agent_manager::AgentManager;
use crate::services::config_service::{ConfigService, KEY_AGENT_AUTO_CONNECT, KEY_AGENT_PATH};
use crate::services::device_service::{AdbEnvironment, AdbRunOutput, AdbRunner};

use super::*;

const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA_OTHER: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn facts() -> ProbeFacts {
    ProbeFacts {
        session_ready: false,
        auto_enabled: true,
        consent_granted: false,
        device_abi: Some("arm64-v8a".into()),
        expected_sha256: Some(SHA_ABC.into()),
        installed_sha256: None,
        agent_running: false,
        failure: None,
    }
}

// ===== 结论：什么事实算成什么结论 =====

#[test]
fn already_in_session_short_circuits_everything_else() {
    let mut f = facts();
    f.session_ready = true;
    // "在跑"、"版本不符"、"产物缺失"一起塞进来：会话已就绪时结论必须是"没事"，
    // 否则设备页每 10s 轮询一次，就会对着一条好端端的会话反复弹按钮。
    f.agent_running = true;
    f.installed_sha256 = Some(SHA_OTHER.into());
    f.failure = Some(ProbeFailure::ArtifactMissing("x".into()));
    assert_eq!(decide(&f).0, AgentProbeDecision::InSession);
    assert_eq!(decide(&f).1, AgentAutoAction::None);
}

#[test]
fn hard_prerequisites_are_blocked_and_never_ask_for_consent() {
    for (failure, expected) in [
        (
            ProbeFailure::ArtifactMissing("没有产物".into()),
            AgentProbeDecision::ArtifactMissing,
        ),
        (
            ProbeFailure::UnsupportedAbi("x86_64".into()),
            AgentProbeDecision::UnsupportedAbi,
        ),
        (
            ProbeFailure::DeviceOffline("unauthorized".into()),
            AgentProbeDecision::DeviceOffline,
        ),
        (
            ProbeFailure::ProbeFailed("adb 挂了".into()),
            AgentProbeDecision::ProbeFailed,
        ),
    ] {
        let mut f = facts();
        f.failure = Some(failure);
        // 授权表里写着"允许"也不许动：前置不满足时，"用户同意过"不能替代事实。
        f.consent_granted = true;
        let (decision, action, detail) = decide(&f);
        assert_eq!(decision, expected);
        assert_eq!(action, AgentAutoAction::Blocked);
        assert!(detail.is_some(), "{expected:?} 必须带一句人能看懂的原因");
    }
}

#[test]
fn artifact_states_map_to_the_three_installed_shapes() {
    let mut current = facts();
    current.installed_sha256 = Some(SHA_ABC.into());
    assert_eq!(decide(&current).0, AgentProbeDecision::IdleArtifactCurrent);

    let mut stale = facts();
    stale.installed_sha256 = Some(SHA_OTHER.into());
    assert_eq!(decide(&stale).0, AgentProbeDecision::StaleArtifact);

    assert_eq!(decide(&facts()).0, AgentProbeDecision::NotInstalled);
}

#[test]
fn missing_hashes_are_reported_instead_of_being_guessed_as_not_installed() {
    // 把 expected 抹掉：以前这种会被当成"没装"，于是一次白推 + 白覆盖安装。
    let mut f = facts();
    f.expected_sha256 = None;
    assert_eq!(decide(&f).0, AgentProbeDecision::ProbeFailed);
    assert_eq!(decide(&f).1, AgentAutoAction::Blocked);
}

#[test]
fn consent_is_the_only_thing_between_asking_and_acting() {
    let mut f = facts();
    assert_eq!(decide(&f).1, AgentAutoAction::AskConsent);
    f.consent_granted = true;
    assert_eq!(decide(&f).1, AgentAutoAction::ConnectingAllowed);
}

#[test]
fn an_agent_we_do_not_become_a_restart_but_an_explicit_takeover() {
    // D063 的那条：Agent 在跑但不是本进程连的。即便已授权（授权的是"可以装/起"，
    // 不是"可以随便重启"），也只给「接管」按钮，不自己杀。
    let mut f = facts();
    f.agent_running = true;
    f.installed_sha256 = Some(SHA_ABC.into());
    f.consent_granted = true;
    let (decision, action, detail) = decide(&f);
    assert_eq!(decision, AgentProbeDecision::RunningElsewhere);
    assert_eq!(action, AgentAutoAction::ExplicitTakeover);
    assert!(detail.unwrap().contains("托管进程"));
}

#[test]
fn auto_disabled_keeps_the_conclusion_but_acts_on_nothing() {
    let mut f = facts();
    f.auto_enabled = false;
    let (decision, action, detail) = decide(&f);
    assert_eq!(decision, AgentProbeDecision::NotInstalled);
    assert_eq!(action, AgentAutoAction::None);
    assert!(detail.unwrap().contains("自动连接已关闭"));
}

// ===== 授权表：一台设备一次，收得回来 =====

fn fresh_config() -> Arc<ConfigService> {
    Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())))
}

#[test]
fn consent_round_trips_and_revoking_the_last_one_clears_the_value() {
    let config = fresh_config();
    let runner = Arc::new(RecordingRunner::default());
    let service = AgentAutoService::new(
        Arc::new(AgentManager::new(
            runner,
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        )),
        config.clone(),
    );
    assert!(!service.has_consent("PIXEL-1"));
    service.set_consent("PIXEL-1", true).unwrap();
    service.set_consent("192.168.1.9:5555", true).unwrap();
    assert!(service.has_consent("PIXEL-1"));
    assert!(service.has_consent("192.168.1.9:5555"));
    assert!(!service.has_consent("VIVO-1"));
    // 重复授权必须幂等：否则同一台设备插拔十次，这个键就被撑成十条重复串
    service.set_consent("PIXEL-1", true).unwrap();
    let stored = config.get(KEY_AGENT_CONSENT_SERIALS, "").unwrap();
    assert_eq!(stored.matches("PIXEL-1").count(), 1, "重复授权：{stored}");
    service.set_consent("PIXEL-1", false).unwrap();
    service.set_consent("192.168.1.9:5555", false).unwrap();
    assert_eq!(config.get(KEY_AGENT_CONSENT_SERIALS, "").unwrap(), "");
    assert!(!service.has_consent("PIXEL-1"));
    // 空 serial 不该进表（它会和"任何设备"比中）
    assert!(service.set_consent("   ", true).is_err());
}

#[test]
fn the_consent_string_we_build_passes_the_settings_validator() {
    // 这条存在的意义：授权串是我们自己拼的。一旦拼出校验不过的形状（末尾多一个逗号
    // 之类），`set` 会当场报错，用户看到的现象就是"我明明点过，下次插线又问我"。
    let config = fresh_config();
    let runner = Arc::new(RecordingRunner::default());
    let service = AgentAutoService::new(
        Arc::new(AgentManager::new(
            runner,
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        )),
        config.clone(),
    );
    for serial in ["PIXEL-1", "emulator-5554", "10.0.0.7:5555", "a_b.c-d"] {
        service.set_consent(serial, true).unwrap();
    }
    let stored = config.get(KEY_AGENT_CONSENT_SERIALS, "").unwrap();
    assert_eq!(stored.split(',').count(), 4, "四条授权都在：{stored}");
    // 而非法形状必须被 ConfigService 拒掉，而不是被我们悄悄写进去
    assert!(
        config
            .set(KEY_AGENT_CONSENT_SERIALS, "ok,坏 空格,bad\nnewline")
            .is_err()
    );
}

// ===== 零写入：三条不该动手的支路，各自钉住"一次都没写" =====

#[derive(Default)]
struct RecordingRunner {
    calls: Mutex<Vec<String>>,
    installed: Mutex<Option<String>>,
    running: Mutex<bool>,
    abi: Mutex<String>,
    /// 有的用例要让"推产物"这一步真的停住（并发合并那条腿要的就是这个中间态）
    push_blocker: std::sync::Mutex<Option<Arc<tokio::sync::Semaphore>>>,
}

impl RecordingRunner {
    fn log(&self) -> String {
        self.calls.lock().unwrap().join("\n")
    }

    fn set_state(&self, installed: Option<&str>, running: bool) {
        *self.installed.lock().unwrap() = installed.map(str::to_owned);
        *self.running.lock().unwrap() = running;
    }

    fn set_abi(&self, abi: &str) {
        *self.abi.lock().unwrap() = abi.to_owned();
    }

    fn set_push_blocker(&self, semaphore: Arc<tokio::sync::Semaphore>) {
        *self.push_blocker.lock().unwrap() = Some(semaphore);
    }
}

#[async_trait]
impl AdbRunner for RecordingRunner {
    async fn run(
        &self,
        _adb_path: &str,
        args: &[String],
        _timeout: std::time::Duration,
    ) -> CoreResult<AdbRunOutput> {
        self.calls.lock().unwrap().push(args.join(" "));
        let stdout = if args.iter().any(|a| a == "get-state") {
            "device\n".to_owned()
        } else if args
            .iter()
            .any(|a| a.contains("getprop ro.product.cpu.abi"))
        {
            let abi = self.abi.lock().unwrap().clone();
            // 空 = 用例没特意改 ABI，按最常见的 arm64-v8a 回；这样 `Default` 就能用
            // derive，而不用每个用例都手工初始化一遍（忘一次就全表红）。
            format!("{}\n", if abi.is_empty() { "arm64-v8a" } else { &abi })
        } else if args.iter().any(|a| a.contains("sha256sum")) {
            // 命中回 "<sha>  <路径>"，没装回空串——和真机那条 `if [ -f ... ]` 同形状
            self.installed
                .lock()
                .unwrap()
                .as_ref()
                .map_or_else(String::new, |sha| {
                    format!("{sha}  /data/local/tmp/app_reverse_tools_agent\n")
                })
        } else if args
            .iter()
            .any(|a| a.contains("pidof app_reverse_tools_agent"))
        {
            if *self.running.lock().unwrap() {
                "running\n".to_owned()
            } else {
                String::new()
            }
        } else {
            // 其余（push / chmod / mv / kill / forward / 起进程）一律"成功但无输出"：
            // 断言看的是**有没有被调用**，不是它返回什么。
            if args.iter().any(|a| a == "push") {
                // 先把闸门取出来再 await：`std::sync::MutexGuard` 跨 await 会让这个
                // future 不 Send，而 AdbRunner 的 future 必须 Send。
                let blocker = self.push_blocker.lock().unwrap().clone();
                if let Some(blocker) = blocker {
                    // 借这次 await 把"正在装机"这个中间态造出来
                    let permit = blocker.acquire().await.expect("测试闸门不该被关闭");
                    drop(permit);
                }
            }
            String::new()
        };
        Ok(AdbRunOutput {
            stdout,
            stderr: String::new(),
            exit_code: Some(0),
        })
    }

    async fn environment(&self) -> AdbEnvironment {
        AdbEnvironment {
            installed: true,
            path: Some("/mock/adb".into()),
            source: Some("mock".into()),
            version: None,
            hint: None,
            probe_error: None,
        }
    }

    fn invalidate_cache(&self) {}
}

/// 只读探测**应该**发的命令，一条不多一条不少。
const READ_ONLY_MARKERS: &[&str] = &["get-state", "getprop", "sha256sum", "pidof"];

fn assert_reads_only(log: &str, why: &str) {
    for marker in [
        "push", "pull", "install", "rm -", "chmod", "mv ", "kill", "forward", "nohup", "tcp:",
    ] {
        assert!(
            !log.contains(marker),
            "{why}：不该出现 `{marker}`。实际发过的命令：\n{log}"
        );
    }
    for line in log.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            READ_ONLY_MARKERS.iter().any(|m| line.contains(m)),
            "{why}：发了一条不在只读白名单里的命令：{line}"
        );
    }
}

struct Harness {
    service: Arc<AgentAutoService>,
    runner: Arc<RecordingRunner>,
    config: Arc<ConfigService>,
    /// 临时产物目录必须活着：resolve 是每次现读文件算 sha 的
    _artifact: tempfile::TempDir,
}

fn harness(auto: bool, consent: &[&str], installed: Option<&str>, running: bool) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("android-agent"), b"abc").unwrap();
    let artifact: PathBuf = directory.path().join("android-agent");
    assert_eq!(sha_of(&artifact), SHA_ABC, "测试基线：产物内容必须是 abc");

    let config = fresh_config();
    config
        .set(KEY_AGENT_PATH, &artifact.display().to_string())
        .unwrap();
    if !auto {
        config.set(KEY_AGENT_AUTO_CONNECT, "false").unwrap();
    }
    let joined = consent.join(",");
    if !joined.is_empty() {
        config.set(KEY_AGENT_CONSENT_SERIALS, &joined).unwrap();
    }
    let runner = Arc::new(RecordingRunner::default());
    runner.set_state(installed, running);
    let service = Arc::new(AgentAutoService::new(
        Arc::new(AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        )),
        config.clone(),
    ));
    Harness {
        service,
        runner,
        config,
        _artifact: directory,
    }
}

fn sha_of(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(std::fs::read(path).unwrap());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn auto_disabled_touches_the_phone_zero_times() {
    let h = harness(false, &[], None, false);
    let run = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(run.outcome, AgentAutoOutcome::Skipped);
    assert_eq!(h.runner.log(), "", "关掉自动连接之后一条 adb 都不该发");
    assert!(!h.service.auto_enabled());
}

#[tokio::test]
async fn no_consent_asks_and_writes_nothing() {
    for (installed, running, expected) in [
        (None, false, AgentProbeDecision::NotInstalled),
        (Some(SHA_OTHER), false, AgentProbeDecision::StaleArtifact),
        (
            Some(SHA_ABC),
            false,
            AgentProbeDecision::IdleArtifactCurrent,
        ),
    ] {
        let h = harness(true, &[], installed, running);
        let run = h.service.auto_connect("PIXEL-1").await;
        assert_eq!(run.probe.decision, expected);
        assert_eq!(
            run.outcome,
            AgentAutoOutcome::AwaitedConsent,
            "{expected:?}"
        );
        assert!(!run.probe.consent_granted);
        assert_reads_only(&h.runner.log(), "未授权时只许探测");
        assert_eq!(h.config.get(KEY_AGENT_CONSENT_SERIALS, "").unwrap(), "");
    }
}

#[tokio::test]
async fn a_foreign_running_agent_is_left_alone_even_with_consent() {
    // D063：这台机已授权过（曾经允许装），但插线时设备上有个不归本进程管的 Agent 在跑。
    // 授权不等于"可以随便重启"——重启会连带打掉托管进程与 frida-server。
    let h = harness(true, &["PIXEL-1"], Some(SHA_ABC), true);
    let run = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(run.probe.decision, AgentProbeDecision::RunningElsewhere);
    assert_eq!(run.outcome, AgentAutoOutcome::DeferredTakeover);
    assert_reads_only(&h.runner.log(), "别人的 Agent 不许杀");
    assert!(
        !h.runner.log().contains("kill"),
        "实际命令：\n{}",
        h.runner.log()
    );
}

#[tokio::test]
async fn consent_is_what_lets_the_write_happen() {
    // 同一台设备、同一份产物状态，唯一区别是授权表。这一条是上一条的反证：
    // 如果 AskConsent 那条支路其实一直在偷偷写，这里就会看到两次 push。
    let h = harness(true, &["PIXEL-1"], None, false);
    let run = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(run.probe.decision, AgentProbeDecision::NotInstalled);
    // 没有真 Agent 应答握手，连接必然失败——这里要的是"它确实去装了"这一步。
    assert_eq!(run.outcome, AgentAutoOutcome::Failed);
    let log = h.runner.log();
    assert!(log.contains("push"), "授权后必须真的推产物：\n{log}");
    // 没有真 Agent 应答，失败点落在推完产物之后的校验/握手；错误原文必须带回来，
    // 不能只剩一个 Failed——界面那句"失败：…"靠的就是它。
    let detail = run.error.clone().unwrap_or_default();
    assert!(!detail.is_empty(), "失败必须带原因原文");
}

#[tokio::test]
async fn one_click_of_install_records_consent_and_the_next_pass_writes() {
    // 复刻 `commands::agent::agent_install` 的两步（先记授权，再连）：这一击就是授权。
    // 断言点在"记完之后，自动那一趟真的会去写"——否则整套授权表只是把用户挡住而已。
    let h = harness(true, &[], None, false);
    h.service.set_consent("PIXEL-1", true).unwrap();
    let run = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(run.outcome, AgentAutoOutcome::Failed); // 没有真 Agent 应答握手
    assert!(run.probe.consent_granted, "点过一次就该记住");
    assert_eq!(
        h.config.get(KEY_AGENT_CONSENT_SERIALS, "").unwrap(),
        "PIXEL-1"
    );
    assert!(h.runner.log().contains("push"), "授权后必须真的推产物");
}

#[tokio::test]
async fn probe_is_what_the_ui_reads_and_caches_its_answer() {
    let h = harness(true, &[], Some(SHA_ABC), false);
    assert!(h.service.cached_probe("PIXEL-1").is_none());
    let probe = h.service.probe("PIXEL-1").await;
    assert_eq!(probe.decision, AgentProbeDecision::IdleArtifactCurrent);
    assert_eq!(probe.device_abi.as_deref(), Some("arm64-v8a"));
    assert_eq!(probe.installed_sha256.as_deref(), Some(SHA_ABC));
    assert_eq!(
        h.service.cached_probe("PIXEL-1").unwrap(),
        probe,
        "诊断读的是同一份缓存，不该每轮都重新探一次"
    );
    assert_reads_only(&h.runner.log(), "probe 永远只读");
}

#[tokio::test]
async fn an_unsupported_abi_is_reported_as_a_reason_not_a_prompt() {
    let h = harness(true, &[], None, false);
    // 设备自报 armeabi-v7a：不是"要不要装"的问题，是本机只有 arm64 产物（AR12.2 未收口）
    let runner = &h.runner;
    runner.set_abi("armeabi-v7a");
    let run = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(run.probe.decision, AgentProbeDecision::UnsupportedAbi);
    assert_eq!(run.probe.action, AgentAutoAction::Blocked);
    assert_eq!(run.outcome, AgentAutoOutcome::Skipped);
    assert!(run.probe.detail.unwrap().contains("arm64-v8a"));
    assert_reads_only(&h.runner.log(), "ABI 不合就不该继续探测产物");
}

#[tokio::test]
async fn missing_artifact_stops_before_the_first_device_call() {
    let directory = tempfile::tempdir().unwrap();
    let config = fresh_config();
    // 指到一个不存在的产物路径：resolve 阶段就该失败，设备一次都不该被碰到
    config
        .set(
            KEY_AGENT_PATH,
            &directory.path().join("nope").display().to_string(),
        )
        .unwrap();
    let runner = Arc::new(RecordingRunner::default());
    let service = AgentAutoService::new(
        Arc::new(AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        )),
        config,
    );
    let run = service.auto_connect("PIXEL-1").await;
    assert_eq!(run.outcome, AgentAutoOutcome::Skipped);
    assert_eq!(run.probe.decision, AgentProbeDecision::ArtifactMissing);
    assert_eq!(runner.log(), "", "产物都没有，不该去问设备任何东西");
}

#[tokio::test]
async fn concurrent_triggers_for_one_serial_are_merged() {
    // 现场：watch 线程刚为这台设备起了一次连接（正在推产物），用户同时又点了按钮。
    // 必须合并成一次，而不是排队再装一遍——第二次装机意味着把同一份产物再推一次、
    // 再覆盖一次，而用户看到的只是"我点了一下"。
    let blocker = Arc::new(tokio::sync::Semaphore::new(0));
    let h = harness(true, &["PIXEL-1"], None, false);
    h.runner.set_push_blocker(blocker.clone());
    let first_service = h.service.clone();
    let first = tokio::spawn(async move { first_service.auto_connect("PIXEL-1").await });
    // 等到第一次真的进到"推产物"那一步，占位已经落下
    for _ in 0..200 {
        if h.runner.log().contains("push") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        h.runner.log().contains("push"),
        "第一次连接该已经走到推产物：\n{}",
        h.runner.log()
    );
    let second = h.service.auto_connect("PIXEL-1").await;
    assert_eq!(second.outcome, AgentAutoOutcome::InFlight);
    assert_eq!(
        h.runner.log().matches("push").count(),
        1,
        "第二次不许重复推产物：\n{}",
        h.runner.log()
    );
    // 放开闸门，让第一次走完；结束后占位必须被清掉，否则这台设备以后再也连不上
    blocker.add_permits(1);
    let first = first.await.unwrap();
    assert_eq!(
        first.outcome,
        AgentAutoOutcome::Failed,
        "没有真 Agent，必然失败"
    );
    let third = h.service.auto_connect("PIXEL-1").await;
    assert_ne!(
        third.outcome,
        AgentAutoOutcome::InFlight,
        "第一次结束之后占位要释放，不能把这台设备永久锁死"
    );
}
