//! 窗口层命令：透明区点击穿透（第五十四轮，用户要求"透明部分能点穿到后面的 App"）。
//!
//! 为什么会误事：窗口是 `transparent: true`，主体颜色用 `rgb(底色 / alpha)` 画
//! （设置里那个透明度滑块调的就是它）。而 macOS 判断"这一点归哪个窗口"看的是
//! 窗口的矩形/形状，**不看像素有多透明**；DOM 同理，只看元素盒子，不看 opacity。
//! 于是留白、卡片之间的间隙、圆角外那些"看得透"的地方照样把点击吃掉。
//!
//! 单窗口要"这块像素既画出半透明颜色、又不接点击"是做不到的：
//! `set_ignore_cursor_events` 是整窗开关，`NSWindow.shape` 挖出来的洞则什么都画不出来。
//! 唯一能两头都要到的办法是**让穿透状态跟着鼠标位置走**：鼠标底下是实体（卡片、控件、
//! 文字、窗口 chrome）就接管点击，是纯透明处就把整窗设成穿透，让点击落到后面的 App。
//!
//! 三条安全底线写在这个文件里，不接受"应该没问题"：
//! ① 标题栏与 tab 栏由前端标成永远实体 → 用户永远点得回来把开关关掉；
//! ② 前端每次询问指针都算一次心跳，Rust 侧看门狗超时未收到心跳就恢复可点击
//!    （否则 JS 卡死/热重载会把整个界面锁成穿透，看起来像"软件坏了"）；
//! ③ 任何取不到指针状态的情况都按「可点击」处理：不知道不等于可以吞掉点击。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::Manager;

use crate::core::error::{CoreError, CoreResult};

/// 主窗口 label（与 `tauri.conf.json` 一致）
const MAIN_WINDOW: &str = "main";
/// 心跳超时：前端超过这个时间没再来问指针位置，就恢复可点击
const HEARTBEAT_TIMEOUT_MS: u64 = 1_500;
/// 看门狗检查间隔（比超时小一个量级，别到点才发现）
const WATCHDOG_TICK: Duration = Duration::from_millis(250);

/// 当前是否处于"穿透"状态（前端关开关时也会显式置回 false）
static CLICK_THROUGH_ON: AtomicBool = AtomicBool::new(false);
/// 最后一次心跳（unix 毫秒）。初值 0 = 从来没心跳过 → 看门狗一上来就会把状态纠正过来
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
/// 看门狗线程只起一次
static WATCHDOG: std::sync::Once = std::sync::Once::new();
/// 线程要用的 AppHandle（第一次开穿透时存下来）
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// 光标相对 webview 左上角的逻辑坐标（CSS px）：前端拿它 `elementFromPoint`。
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PointerState {
    pub x: f64,
    pub y: f64,
    /// 光标在不在这个窗口范围内。不在时前端**不改**穿透状态：鼠标在别处，
    /// 我们这儿穿不穿透都跟它要点的东西无关，来回开关只会闪。
    pub inside: bool,
}

/// 物理屏幕坐标 → webview 内逻辑坐标（纯函数，Retina 与负坐标都能单独钉住）。
///
/// 副屏可以在主屏左边/上边，`outer_position` 因此**允许是负数**；
/// 这里全程用 f64，不做无符号转换，否则多显示器下整窗会被判成"光标在外面"。
pub fn pointer_state(
    cursor: (f64, f64),
    outer: (f64, f64),
    inner_physical: (f64, f64),
    scale: f64,
) -> PointerState {
    // scale 读不到时按 1 处理：宁可坐标算歪一点，也不要 panics 或把窗口判成不可达
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let x = (cursor.0 - outer.0) / scale;
    let y = (cursor.1 - outer.1) / scale;
    let width = inner_physical.0 / scale;
    let height = inner_physical.1 / scale;
    PointerState {
        x,
        y,
        inside: x >= 0.0 && y >= 0.0 && x < width && y < height,
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn set_ignore(app: &tauri::AppHandle, ignore: bool) -> CoreResult<()> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| CoreError::Internal(format!("找不到主窗口 {MAIN_WINDOW}")))?;
    // 只在状态真要变的时候打：这条命令只在跳变时被前端调用，日志不会刷屏。
    // 现场出问题时（"点不到了"还是"没穿透"），dev 终端里这一行是唯一能对齐的证据。
    if CLICK_THROUGH_ON.load(Ordering::SeqCst) != ignore {
        eprintln!("audit method=window_set_click_through ignore={ignore}");
    }
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|error| CoreError::Internal(format!("设置点击穿透失败: {error}")))
}

/// 心跳：前端每问一次指针就续一期。
fn beat() {
    LAST_BEAT_MS.store(now_millis(), Ordering::SeqCst);
}

/// 开穿透时挂看门狗：JS 卡死/热重载/页面被关，都必须自己变回可点击。
fn arm_watchdog(app: &tauri::AppHandle) {
    let _ = APP.set(app.clone());
    WATCHDOG.call_once(|| {
        let Some(app) = APP.get() else { return };
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(WATCHDOG_TICK);
            if !CLICK_THROUGH_ON.load(Ordering::SeqCst) {
                continue;
            }
            let silent = now_millis().saturating_sub(LAST_BEAT_MS.load(Ordering::SeqCst));
            if silent <= HEARTBEAT_TIMEOUT_MS {
                continue;
            }
            match set_ignore(&app, false) {
                Ok(()) => {
                    CLICK_THROUGH_ON.store(false, Ordering::SeqCst);
                    eprintln!(
                        "[click-through] {silent}ms 没有前端心跳，判定 JS 已停，已恢复窗口可点击"
                    );
                }
                Err(error) => eprintln!("[click-through] 恢复可点击失败: {error}"),
            }
        });
    });
}

/// 取指针位置（相对本窗口，逻辑像素）。同时续一次心跳。
#[tauri::command]
pub fn window_pointer_state(app: tauri::AppHandle) -> CoreResult<PointerState> {
    beat();
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| CoreError::Internal(format!("找不到主窗口 {MAIN_WINDOW}")))?;
    let scale = window
        .scale_factor()
        .map_err(|error| CoreError::Internal(format!("读缩放比失败: {error}")))?;
    let cursor = window
        .cursor_position()
        .map_err(|error| CoreError::Internal(format!("读指针位置失败: {error}")))?;
    let outer = window
        .outer_position()
        .map_err(|error| CoreError::Internal(format!("读窗口位置失败: {error}")))?;
    let inner = window
        .inner_size()
        .map_err(|error| CoreError::Internal(format!("读窗口尺寸失败: {error}")))?;
    Ok(pointer_state(
        (cursor.x, cursor.y),
        (outer.x as f64, outer.y as f64),
        (inner.width as f64, inner.height as f64),
        scale,
    ))
}

/// 切换穿透。返回真正落下去的状态：调用方据此同步，不拿"我以为设上了"当事实。
#[tauri::command]
pub fn window_set_click_through(app: tauri::AppHandle, ignore: bool) -> CoreResult<bool> {
    beat();
    set_ignore(&app, ignore)?;
    CLICK_THROUGH_ON.store(ignore, Ordering::SeqCst);
    if ignore {
        arm_watchdog(&app);
    }
    Ok(ignore)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_state_maps_retina_pixels_to_css_pixels() {
        // 2x Retina：窗口在 (100,100) 物理像素，内容 1600x1000 物理像素 = 800x500 逻辑
        let state = pointer_state((500.0, 300.0), (100.0, 100.0), (1600.0, 1000.0), 2.0);
        assert_eq!(
            (state.x, state.y),
            (200.0, 100.0),
            "要按 CSS px 交给 elementFromPoint"
        );
        assert!(state.inside);
    }

    #[test]
    fn pointer_state_handles_secondaries_left_of_main_display() {
        // 副屏在主屏左边：outer_position 是负数。用无符号承接会把整窗判成"光标在外面"
        let state = pointer_state((-800.0, 200.0), (-1200.0, 100.0), (1200.0, 800.0), 1.0);
        assert_eq!((state.x, state.y), (400.0, 100.0));
        assert!(state.inside, "负坐标窗口里的点必须算在里面");
    }

    #[test]
    fn pointer_outside_the_window_is_flagged_not_guessed() {
        let outside = pointer_state((900.0, 900.0), (0.0, 0.0), (800.0, 600.0), 1.0);
        assert!(!outside.inside);
        // 右/下边界是开区间：正好压边的点归下一个窗口，别抢
        let edge = pointer_state((800.0, 300.0), (0.0, 0.0), (800.0, 600.0), 1.0);
        assert!(!edge.inside);
    }

    #[test]
    fn broken_scale_falls_back_to_one_instead_of_poisoning_the_math() {
        for scale in [0.0, -1.0, f64::NAN] {
            let state = pointer_state((20.0, 30.0), (0.0, 0.0), (800.0, 600.0), scale);
            assert_eq!((state.x, state.y), (20.0, 30.0), "scale={scale}");
            assert!(state.inside);
        }
    }
}
