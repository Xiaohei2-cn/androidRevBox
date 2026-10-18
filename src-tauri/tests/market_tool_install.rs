//! 工具市场真机腿（P13）：`market_install_tool` 的完整链路。
//!
//! 跑法：
//! ```bash
//! MARKET_TEST_SERIAL=<serial> cargo test -p app-reverse-tools --test market_tool_install \
//!   -- --ignored --nocapture
//! ```
//!
//! 为什么值得有一条真机腿：这条链路上有四处只有设备能回答——
//! ① GitHub 的 release 资产是否还是登记的摘要；② `adb push` 是否真的落到了
//! `/data/local/tmp/`；③ `chmod 0755` 之后是否真的可执行；④ 回读的体积是否一致。
//! 任何一处断掉，界面上的"已安装"就是假的，而这些在单测里都是 mock。

use std::sync::Arc;

use app_reverse_tools_lib::services::config_service::ConfigService;
use app_reverse_tools_lib::db::Db;
use app_reverse_tools_lib::services::device_service::{AdbRunner, RealAdbRunner};
use app_reverse_tools_lib::services::tool_market::ToolMarketService;

#[tokio::test]
#[ignore = "需要真机；MARKET_TEST_SERIAL=<serial> cargo test -p app-reverse-tools --test market_tool_install -- --ignored --nocapture"]
async fn real_market_installs_stackplz_to_local_tmp() {
    let serial = std::env::var("MARKET_TEST_SERIAL")
        .expect("MARKET_TEST_SERIAL is required（这条腿会把二进制推到真机）");
    // 集成测试不改 `Db::in_memory` 的可见性（那是 `#[cfg(test)]` 的库内单测件）：
    // 用一个临时目录开真库，与 App 启动时走同一条路径。
    let data_dir = std::env::temp_dir().join(format!("art-market-db-{}", std::process::id()));
    let config = Arc::new(ConfigService::new(Arc::new(Db::connect(&data_dir).unwrap())));
    let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config));
    let cache = std::env::temp_dir().join("art-market-real-test");
    let service = ToolMarketService::new(runner, cache);

    let report = service
        .install(&serial, "stackplz")
        .await
        .expect("安装失败");

    eprintln!(
        "[market] {} {} -> {} ({} 字节, 复核 {:?}, exec={})",
        report.id,
        report.version,
        report.device_path,
        report.bytes,
        report.verified_bytes,
        report.executable
    );

    assert_eq!(report.device_path, "/data/local/tmp/stackplz");
    // 设备侧必须真的可执行：不可执行的话，"装上了"只是文件躺在那里
    assert!(report.executable, "推送后没有执行权限");
    // 复核成功时体积必须与目录登记的一致；复核失败（None）只记录，不当成失败
    if let Some(verified) = report.verified_bytes {
        assert_eq!(verified, report.bytes, "设备侧字节数与下载的不一致");
    }
}
