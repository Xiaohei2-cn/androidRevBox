//! 客户端在场记录与"没人用了就自己退场"（AR12.7 / 决策 D088）。
//!
//! 要解决的问题只有这一个：桌面进程死了（或被重启）之后，设备上的 Agent 还活着，
//! 而下一个桌面会话**分不清"这个 Agent 是活的实例在用"还是"是上一任死掉留下的"**
//! （`is_running` 只看进程在不在）。于是它只能不动手，用户每重启一次 App 就要人手点一次
//! 「重启并接管」。
//!
//! 为什么由 Agent 自己解：客户端连没连，**只有 Agent 知道**——它手上那条已鉴权的 TCP/Unix
//! 连接就是事实本身。桌面侧任何推断（owner 标记文件、比 mtime、pid 表、心跳）都是在猜，
//! 猜错一次要么误杀正在用的 Agent，要么照样接不回来。
//!
//! 两条硬边界，写在这儿是因为它们比"会退场"更容易被后人改错：
//! 1. **只有已鉴权的连接算在场**。握手失败的连接（令牌不对、协议不对）既不延长也不取消宽限，
//!    否则任何人拿个 socket 连一下就能让 Agent 永远不死，或反过来被无关连接挤掉。
//! 2. **退场不带走孩子**。托管进程与 frida-server 是 Agent `spawn` 的、跑在**独立进程组**里
//!    （`provider::hosted` 起进程时显式 `process_group(0)`，注释就写着"Agent 停止时不把托管进程
//!    一起带走"），输出落在设备上的 `.name.run.log` 里。Agent 退出后它们继续跑，
//!    新 Agent 起来由桌面侧 `hosted.adopt` 重新认领。**如果哪天有人给这里加了"退出前杀孩子"，
//!    就等于把「重启工具」变成「干掉正在分析的目标进程」**，那是比现在更糟的行为。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// 已鉴权客户端的在场计数。`Drop` 负责减，所以 `serve_connection` 里那条连接
/// 不管从哪条路径离开（EOF、错误、被 abort）都不会把计数漏在外面。
#[derive(Debug, Default)]
pub struct ClientPresence {
    live: Arc<AtomicUsize>,
}

impl ClientPresence {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个已鉴权客户端。返回的守卫活着 = 这个客户端在场。
    pub fn attend(&self) -> Attendance {
        self.live.fetch_add(1, Ordering::SeqCst);
        Attendance {
            live: self.live.clone(),
        }
    }

    /// 当前在场数（0 = 没人用）。
    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }
}

/// RAII 守卫：连接结束即扣减。
#[derive(Debug)]
pub struct Attendance {
    live: Arc<AtomicUsize>,
}

impl Drop for Attendance {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 「连续没人用够宽限期」这台状态机。抽出来是因为它必须能脱离 tokio 被单测——
/// "中途来过一个客户端就要重新计时"这种事，用真时间测既慢又容易假绿。
#[derive(Debug, Default)]
pub struct IdleExit {
    zero_since: Option<Instant>,
}

impl IdleExit {
    /// 每轮喂一次当前在场数。返回 `true` = 该退场了。
    ///
    /// 判据是**连续**为空满宽限期：中途 `live` 变成非零就把计时清零。
    /// 用累计空时会产生一个真故障——桌面在宽限期内重连了一次又掉线，
    /// 累计时间早就够，Agent 会在第二个客户端刚握完手就自己走掉。
    pub fn observe(&mut self, live: usize, grace: Duration, now: Instant) -> bool {
        if live > 0 {
            self.zero_since = None;
            return false;
        }
        let since = *self.zero_since.get_or_insert(now);
        now.duration_since(since) >= grace
    }

    /// 已经空了多久（诊断/日志用）。
    pub fn idle_for(&self, now: Instant) -> Option<Duration> {
        self.zero_since.map(|since| now.duration_since(since))
    }
}

/// 宽限期轮询间隔。0.25 s 相对 30 s 的宽限完全够，而不用去赌 Notify 的注册时机
/// （漏一次唤醒就是"该退没退"或"不该退退了"，那种 bug 只在真机上出现）。
pub const IDLE_POLL: Duration = Duration::from_millis(250);

/// 轮询直到该退场。返回时调用方应当**正常退出进程**（不是 `kill -9` 自己）。
pub async fn wait_until_should_exit(presence: Arc<ClientPresence>, grace: Duration) {
    let mut machine = IdleExit::default();
    loop {
        if machine.observe(presence.live(), grace, Instant::now()) {
            return;
        }
        tokio::time::sleep(IDLE_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[test]
    fn presence_counts_attended_connections_and_drops_on_release() {
        let presence = ClientPresence::new();
        assert_eq!(presence.live(), 0);
        let first = presence.attend();
        let second = presence.attend();
        assert_eq!(presence.live(), 2, "两个已鉴权客户端都在场");
        drop(first);
        assert_eq!(presence.live(), 1);
        drop(second);
        assert_eq!(
            presence.live(),
            0,
            "守卫全部释放后必须归零，否则 Agent 永远不退出"
        );
    }

    #[test]
    fn exits_only_after_the_grace_window_has_been_empty_continuous() {
        let grace = Duration::from_millis(100);
        let start = Instant::now();
        let mut machine = IdleExit::default();
        assert!(!machine.observe(0, grace, start), "刚开始还不该退");
        assert!(
            !machine.observe(0, grace, at(60)),
            "宽限期内（60ms<100ms）不许退——重启一次 App 就在这窗口里"
        );
        assert!(machine.observe(0, grace, at(140)), "连续空过宽限期就该退");
    }

    /// 这条是"误杀正在用的 Agent"的护栏：中途来过一个客户端，计时必须从零开始。
    #[test]
    fn a_client_arriving_resets_the_grace_window() {
        let grace = Duration::from_millis(100);
        let mut machine = IdleExit::default();
        assert!(!machine.observe(0, grace, Instant::now()));
        assert!(!machine.observe(1, grace, at(80)), "有人用就不算空闲");
        assert!(!machine.observe(1, grace, at(300)), "有人用多久都不该退");
        // 那个客户端走了：宽限期重新起算，不是把之前空过的 80ms 也累计进来
        assert!(!machine.observe(0, grace, at(320)));
        assert!(!machine.observe(0, grace, at(380)));
        assert!(machine.observe(0, grace, at(430)));
    }

    #[test]
    fn idle_duration_is_reported_for_the_log_line() {
        let mut machine = IdleExit::default();
        let start = Instant::now();
        machine.observe(0, Duration::from_secs(30), start);
        let idle = machine
            .idle_for(at(1000))
            .expect("应该记得从什么时候开始空");
        assert!((0.9..1.2).contains(&idle.as_secs_f64()), "{idle:?}");
    }

    #[tokio::test]
    async fn wait_until_should_exit_returns_after_grace_when_nobody_comes() {
        let presence = Arc::new(ClientPresence::new());
        let started = Instant::now();
        wait_until_should_exit(presence.clone(), Duration::from_millis(60)).await;
        assert!(started.elapsed() >= Duration::from_millis(60));
    }

    #[tokio::test]
    async fn wait_until_should_exit_does_not_return_while_a_client_is_present() {
        let presence = Arc::new(ClientPresence::new());
        let guard = presence.attend();
        let waiter = {
            let presence = presence.clone();
            tokio::spawn(async move {
                wait_until_should_exit(presence, Duration::from_millis(40)).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !waiter.is_finished(),
            "有已鉴权客户端在场时，Agent 不许自己退场（那就是把正在用的人踢掉）"
        );
        drop(guard);
        waiter.await.unwrap();
    }
}
