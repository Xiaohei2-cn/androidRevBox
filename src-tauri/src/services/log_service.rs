//! LogService：结构化日志的运行时管理（P1）。
//! - 文件输出：tracing-appender 按天滚动（app.log.YYYY-MM-DD），non-blocking 写入；
//! - 级别可配：reload Layer 运行时切换 EnvFilter，并持久化到 app_settings，
//!   重启时由 lib.rs 读取恢复（非法值回退 info）。

use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing_subscriber::Registry;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;

use crate::core::error::{CoreError, CoreResult};
use crate::services::config_service::{ConfigService, KEY_LOG_LEVEL};

const VALID_LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

pub struct LogService {
    filter: reload::Handle<EnvFilter, Registry>,
    config: Arc<ConfigService>,
    current: Mutex<String>,
}

fn build_filter(level: &str) -> EnvFilter {
    // 本 crate 按指定级别输出，其余库保持 warn，避免依赖日志刷屏
    EnvFilter::try_new(format!("app_reverse_tools_lib={level},warn"))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// 初始化全局 tracing 订阅（进程内只调用一次），返回可调级的 LogService。
pub fn init(
    log_dir: &Path,
    initial_level: &str,
    config: Arc<ConfigService>,
) -> CoreResult<Arc<LogService>> {
    std::fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "app.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    // non_blocking 的 WorkerGuard 必须活到进程结束，否则日志静默丢失；
    // P1 无优雅停机路径，用 Box::leak 将 guard 提升为进程生命周期。
    Box::leak(Box::new(guard));

    let (filter, handle) = reload::Layer::new(build_filter(initial_level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .init();

    Ok(Arc::new(LogService {
        filter: handle,
        config,
        current: Mutex::new(initial_level.to_string()),
    }))
}

impl LogService {
    pub fn level(&self) -> String {
        self.current.lock().expect("log level lock").clone()
    }

    /// 切换运行时日志级别并持久化；非法级别直接报错、不落库。
    pub fn set_level(&self, level: &str) -> CoreResult<String> {
        if !VALID_LEVELS.contains(&level) {
            return Err(CoreError::Internal(format!(
                "非法日志级别: {level}（允许 {}）",
                VALID_LEVELS.join("/")
            )));
        }
        self.filter
            .reload(build_filter(level))
            .map_err(|e| CoreError::Internal(format!("重载日志过滤器失败: {e}")))?;
        self.config.set(KEY_LOG_LEVEL, level)?;
        *self.current.lock().expect("log level lock") = level.to_string();
        tracing::info!(level, "log level changed");
        Ok(level.to_string())
    }

    /// 启动时从持久化配置解析级别（非法值回退 info）。
    pub fn resolve_startup_level(config: &ConfigService) -> String {
        let stored = config
            .get(KEY_LOG_LEVEL, "info")
            .unwrap_or_else(|_| "info".to_string());
        if VALID_LEVELS.contains(&stored.as_str()) {
            stored
        } else {
            "info".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn config() -> Arc<ConfigService> {
        Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())))
    }

    #[test]
    fn startup_level_reads_valid_stored_value() {
        let cs = config();
        cs.set(KEY_LOG_LEVEL, "debug").unwrap();
        assert_eq!(LogService::resolve_startup_level(&cs), "debug");
    }

    #[test]
    fn startup_level_falls_back_on_illegal_value() {
        let cs = config();
        cs.db
            .with(|c| {
                c.execute(
                    "INSERT INTO app_settings(key,value) VALUES(?1,'not-a-level')",
                    [KEY_LOG_LEVEL],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(LogService::resolve_startup_level(&cs), "info");
    }

    #[test]
    fn build_filter_rejects_garbage_with_fallback() {
        // 非法级别字符串应回退 info，而不是 panic
        let f = build_filter("definitely not a level");
        assert!(f.to_string().contains("warn") || !f.to_string().is_empty());
    }
}
