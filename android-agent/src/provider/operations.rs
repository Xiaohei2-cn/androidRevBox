//! 写操作幂等台账（AR8.1）。
//!
//! 三个包写操作（`activity.launch`、`activity.force_stop`、`package.uninstall`）共用一份
//! `operation_id → 已完成结果` 的台账：网络重试、用户连点、Desktop 崩溃后重发，
//! 都只能拿到「上次做过什么」的复用结果，不能再动一次设备状态——§3.6 要的就是这条。
//!
//! 台账只在 Agent 进程内存活（重启即清空），所以 `operation_id` 由 Desktop 每次**操作**
//! 生成、而不是每次**请求**生成；跨 Agent 重启的重发会真的再执行一次，这是有意的边界：
//! 想要跨重启的幂等就得把台账落盘并处理 PID/包状态变化，那是 AR8.4 的原子替换才需要的强度。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

const LEDGER_TTL: Duration = Duration::from_secs(10 * 60);
const LEDGER_CAP: usize = 256;

#[derive(Default)]
struct Ledger {
    entries: HashMap<String, (Instant, Value)>,
}

static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();

fn ledger() -> &'static Mutex<Ledger> {
    LEDGER.get_or_init(|| Mutex::new(Ledger::default()))
}

/// `operation_id` 只允许稳定可打印字符，长度受限：它会进日志与 map key，
/// 不能成为注入或内存膨胀的入口。
pub(crate) fn is_valid_operation_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 64
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// 命中已有结果时返回上次的响应（由调用方改写成 `replayed`）。
pub(crate) fn lookup(operation_id: &str) -> Option<Value> {
    let mut guard = ledger()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = lookup_at(&mut guard, Instant::now(), operation_id);
    prune(&mut guard, Instant::now());
    value
}

fn lookup_at(ledger: &mut Ledger, now: Instant, operation_id: &str) -> Option<Value> {
    let (at, value) = ledger.entries.get(operation_id)?;
    // 超时的条目当成没有：宁可重新执行一次，也不拿十分钟前的结果糊弄新操作
    if now.saturating_duration_since(*at) > LEDGER_TTL {
        return None;
    }
    Some(value.clone())
}

pub(crate) fn remember(operation_id: &str, value: &Value) {
    let mut guard = ledger()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    guard
        .entries
        .insert(operation_id.to_owned(), (now, value.clone()));
    prune(&mut guard, now);
}

/// 复用上次结果时把 outcome 改写成 `replayed`：调用方必须能看出这次没有再执行。
/// 用 JSON 层改写，避免三个方法共用一个具体 DTO 类型。
pub(crate) fn mark_replayed(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("outcome".to_owned(), Value::String("replayed".to_owned()));
    }
    value
}

/// 先按 TTL 淘汰，再按数量淘汰最旧的：只留最近 `LEDGER_CAP` 条。
fn prune(ledger: &mut Ledger, now: Instant) {
    ledger
        .entries
        .retain(|_, (at, _)| now.saturating_duration_since(*at) <= LEDGER_TTL);
    if ledger.entries.len() <= LEDGER_CAP {
        return;
    }
    let mut ordered: Vec<(Instant, String)> = ledger
        .entries
        .iter()
        .map(|(id, (at, _))| (*at, id.clone()))
        .collect();
    ordered.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, id) in ordered.iter().skip(LEDGER_CAP) {
        ledger.entries.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_validated_before_becoming_map_keys() {
        for bad in [
            "",
            "with space",
            "a;b",
            "$(id)",
            "x".repeat(65).as_str(),
            "带中文",
        ] {
            assert!(!is_valid_operation_id(bad), "{bad:?} 必须被拒");
        }
        for good in ["op-1", "desktop:uuid:1", "a.b_c:d", "x".repeat(64).as_str()] {
            assert!(is_valid_operation_id(good), "{good:?} 应该合法");
        }
    }

    #[test]
    fn ledger_lookup_respects_ttl_and_is_bounded() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let fresh = now;
        let stale = now - LEDGER_TTL - Duration::from_secs(1);
        ledger
            .entries
            .insert("fresh".into(), (fresh, Value::from(1)));
        ledger
            .entries
            .insert("stale".into(), (stale, Value::from(2)));
        assert_eq!(lookup_at(&mut ledger, now, "fresh"), Some(Value::from(1)));
        assert_eq!(
            lookup_at(&mut ledger, now, "stale"),
            None,
            "过期条目不能当命中"
        );
        assert_eq!(lookup_at(&mut ledger, now, "absent"), None);

        for index in 0..(LEDGER_CAP + 40) {
            ledger
                .entries
                .insert(format!("id-{index}"), (now, Value::from(index)));
        }
        prune(&mut ledger, now);
        assert_eq!(ledger.entries.len(), LEDGER_CAP, "台账必须有界");
    }

    #[test]
    fn replayed_marker_only_touches_the_outcome_field() {
        let stored = serde_json::json!({
            "action": "launch",
            "package": "com.x",
            "operation_id": "op-1",
            "outcome": "executed",
            "verified": true,
            "ran_as_root": false
        });
        let replayed = mark_replayed(stored.clone());
        assert_eq!(replayed["outcome"], "replayed");
        assert_eq!(replayed["verified"], stored["verified"]);
        assert_eq!(replayed["package"], stored["package"]);
        // 非对象报文不该被改写坏
        assert_eq!(mark_replayed(Value::from(7)), Value::from(7));
    }

    #[test]
    fn remember_then_lookup_round_trip() {
        let id = format!("test-{}", Instant::now().elapsed().as_nanos());
        assert_eq!(lookup(&id), None);
        remember(&id, &Value::from("done"));
        assert_eq!(lookup(&id), Some(Value::from("done")));
    }
}
