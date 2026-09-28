//! 宽限期这个数字**必须两边同源**（AR12.7 / D088）。
//!
//! 桌面侧拿它决定"看到有人在跑之后多久回来看一眼"，Agent 侧拿它决定"多久没人用就自己走"。
//! 任何一边单独改，表现都是"还是得人手点一次接管"或者"正在用的 Agent 被踢掉"——
//! 两种都只会在真机上被人撞到，不会在编译期报错。所以把它变成一条会红的扫描。

use std::path::PathBuf;

/// 两处都写死的常量名与期望值（改数字要同时改两处，并回看 §10 的那条现场记录）
const DESKTOP_CONST: &str = "AGENT_IDLE_EXIT_SECS";
const AGENT_CONST: &str = "DEFAULT_IDLE_EXIT_SECS";
const EXPECTED_SECS: &str = "30";
const FLAG: &str = "--idle-exit-secs";

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("读不到 {path:?}（这条扫描本身就失效了，别让它假装通过）：{error}")
    })
}

fn declared_constant(source: &str, name: &str) -> String {
    for line in source.lines() {
        let mut pattern = String::from(name);
        pattern.push_str(": u64 = ");
        if let Some(at) = line.find(&pattern) {
            let tail = &line[at + pattern.len()..];
            return tail.split(';').next().unwrap_or("").trim().to_owned();
        }
    }
    panic!("找不到常量 {name} 的定义——它一旦被改名/删掉，两边同源就悄悄没了");
}

#[test]
fn desktop_and_agent_agree_on_the_idle_exit_grace() {
    let desktop = read("src/adapters/agent_bootstrap.rs");
    let agent = read("../android-agent/src/main.rs");
    let desktop_value = declared_constant(&desktop, DESKTOP_CONST);
    let agent_value = declared_constant(&agent, AGENT_CONST);
    assert_eq!(
        desktop_value, agent_value,
        "宽限期两边不一致：桌面 {DESKTOP_CONST}={desktop_value}，Agent {AGENT_CONST}={agent_value}。"
    );
    assert_eq!(
        desktop_value, EXPECTED_SECS,
        "改动这个数字要先回看 §10 那条真机现场与 §5 的复看余量，别顺手调"
    );
}

#[test]
fn desktop_actually_tells_the_agent_the_grace_and_agent_accepts_the_flag() {
    let desktop = read("src/adapters/agent_bootstrap.rs");
    let agent = read("../android-agent/src/main.rs");
    assert!(
        desktop.contains(FLAG),
        "桌面启动 Agent 时必须把宽限期传下去（否则 Agent 用自己的默认值，桌面的复看时间就对不上）"
    );
    assert!(
        agent.contains(FLAG.trim_start_matches('-')),
        "Agent 必须认这个参数名，否则它会在参数解析处直接起不来"
    );
}

#[test]
fn takeover_recheck_waits_longer_than_the_grace_itself() {
    // 复看必须晚于 Agent 自己退场的时刻，否则回去看还是"有人在跑"，白等一轮。
    let desktop = read("src/adapters/agent_bootstrap.rs");
    assert!(
        desktop.contains("agent_idle_exit_secs() + 15"),
        "复看延迟 = 宽限期 + 15s 余量；改这一处必须同时解释余量为什么够"
    );
}
