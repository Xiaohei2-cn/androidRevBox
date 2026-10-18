//! 工具市场（P13）：内置工具目录 + 一键安装到设备。
//!
//! 边界（与 Zygisk 那层同源的纪律）：
//! - **目录是代码里的常量，不是网络上的清单**。每一项的版本、URL、大小、SHA-256 都写死，
//!   界面读到的就是我们要装的那个东西；不存在"服务端换了包，客户端不知道"这种情况。
//! - 下载只走 HTTPS，落盘前校验大小与摘要；校验不过就删掉，绝不把一个来路不明的
//!   ELF 推到设备上。
//! - 推送目标固定 `/data/local/tmp/`（adb shell 自己就能读写的地方），不写系统分区、
//!   不需要 root。装完 `chmod +x`，然后它就是一个普通命令行工具，交给二进制托管页去跑。
//!
//! 为什么不做"在线抓 GitHub API 取最新版"：那样界面显示的是"远端有什么"，
//! 而真正要保证的是"推下去的那一个字节就是我们要的那个"。版本升级是改这里的一行常量 +
//! 一次真机验证，不是让用户在不知情的情况下换掉设备上的二进制。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::adapters::adb;
use crate::core::error::{CoreError, CoreResult};
use crate::services::device_service::{AdbRunner, AdbRunOutput};

/// 设备侧目标目录：adb shell 可写、不需要 root、重启后仍在（`/data/local/tmp` 不清）。
pub const DEVICE_TMP: &str = "/data/local/tmp";
/// 下载超时：9 MB 在正常网络下几秒；给足 5 分钟是为了慢网与 GitHub 的跳转重定向。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// 推送超时：9 MB 走 USB 是秒级，留 3 分钟挡"adb 卡死"。
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);
/// 单次下载的硬上限：防止对端把 Content-Length 说成 1 GB 时把内存/磁盘吃光。
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// 目录里的一项工具。
///
/// 字段都是"装之前就能核对的事实"：装完界面要能说清"装的是哪个版本、多大、推到了哪"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEntry {
    pub id: &'static str,
    pub name: &'static str,
    /// 一句话说明它是什么（界面左侧气泡卡片的副标题）
    pub tagline: &'static str,
    pub description: &'static str,
    /// 上游仓库（官方地址）
    pub homepage: &'static str,
    pub license: &'static str,
    pub version: &'static str,
    /// 下载地址（固定到具体 release，不用 latest——latest 会随上游变动）
    pub download_url: &'static str,
    /// 期望的字节数；下载后必须一致
    pub size_bytes: u64,
    /// 期望的 SHA-256（小写 hex）
    pub sha256: &'static str,
    /// 推到设备后的文件名
    pub device_file_name: &'static str,
    /// 支持的设备 ABI（空 = 与架构无关）
    pub device_abi: &'static [&'static str],
    /// 用法示例，界面上原样展示（只读文本，不可执行）
    pub usage: &'static [&'static str],
}

/// 内置目录。
///
/// 目前只有 stackplz：syscall / 硬件断点追踪，配合 frida 联动使用。
/// 它的 release 只有一个 arm64 静态 ELF（`stackplz`），没有多架构分支。
pub const TOOLS: &[ToolEntry] = &[ToolEntry {
    id: "stackplz",
    name: "stackplz",
    tagline: "syscall 与硬件断点追踪，可配合 frida 联动",
    description: "基于 eBPF 的 Android 追踪工具：不改包、不注入，记录目标进程的 syscall 调用栈，\
也支持硬件断点与 frida 联动。适合定位「哪个调用读了这个文件、连了这个地址」这类问题，\
产物直出，不需要在设备上部署内核模块。",
    homepage: "https://github.com/SeeFlowerX/stackplz",
    license: "GPL-3.0",
    version: "v3.0.3",
    download_url: "https://github.com/SeeFlowerX/stackplz/releases/download/v3.0.3/stackplz",
    size_bytes: 8_975_584,
    // 2026-10-18 从 release 资产实算（`shasum -a 256`）。上游若是替换了同名资产，
    // 这里会挡住并在错误里说清"摘要对不上"，而不是把新东西当旧版本推下去。
    sha256: "f0e2301dfd02e1fa8c2dde507ff8da884f22f7ab646dfe1f317111d9f4afc957",
    device_file_name: "stackplz",
    device_abi: &["arm64-v8a"],
    usage: &[
        // 三条都按 v3.0.3 的 `-h` 实际输出核对过（不要写"看起来像"的参数）：
        // `-n/--name` 要 uid 或包名；`-p/--pid` 是 pid 白名单字符串；
        // `-s/--syscall` 过滤系统调用；`-c/--config` 是 hook 配置数组。
        "stackplz -n com.example.app -s openat",
        "stackplz -n com.example.app -s connect -o /data/local/tmp/trace.log",
        "stackplz -c config.json -s connect",
    ],
}];

/// 一次安装的结果：装了什么、在哪、多大。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInstallReport {
    pub id: String,
    pub version: String,
    pub serial: String,
    /// 设备侧绝对路径（界面直接显示，方便复制到命令行）
    pub device_path: String,
    pub bytes: u64,
    pub sha256: String,
    /// 设备上 `ls -l` 复核出来的字节数；与 `bytes` 不一致说明推送不完整
    pub verified_bytes: Option<u64>,
    /// 设备上是否已可执行（`chmod +x` 之后 `test -x`）
    pub executable: bool,
}

pub struct ToolMarketService {
    runner: Arc<dyn AdbRunner>,
    /// 下载缓存目录（本机 app data 下的 tools/），重复安装同一版本不必再下一次
    cache_dir: PathBuf,
}

impl ToolMarketService {
    /// `cache_dir` 由调用方给（`lib.rs` 用 app data 目录）：这一层不需要知道
    /// ConfigService 的内部结构，测试也能直接指到临时目录。
    pub fn new(runner: Arc<dyn AdbRunner>, cache_dir: PathBuf) -> Self {
        Self { runner, cache_dir }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn catalog(&self) -> &'static [ToolEntry] {
        TOOLS
    }

    pub fn entry(&self, id: &str) -> Option<&'static ToolEntry> {
        TOOLS.iter().find(|tool| tool.id == id)
    }

    /// 本机缓存路径。文件名带版本与摘要前 12 位：升级版本后旧文件自然不再被命中，
    /// 也不会因为"同名文件已存在"把新版本当成旧的。
    pub fn cache_path(&self, tool: &ToolEntry) -> PathBuf {
        let tag = &tool.sha256[..12.min(tool.sha256.len())];
        self.cache_dir
            .join(format!("{}-{}-{}", tool.id, tool.version, tag))
    }

    /// 确保本机有一份**校验过**的二进制，返回它的路径。
    ///
    /// 缓存存在且摘要一致就直接用；不一致就删掉重下——缓存是加速手段，不是可信来源，
    /// 它的正确性永远由摘要判定，而不是由"文件在那里"判定。
    pub async fn ensure_local(&self, tool: &ToolEntry) -> CoreResult<PathBuf> {
        let path = self.cache_path(tool);
        if path.is_file() {
            if let Ok(actual) = sha256_file(&path).await
                && actual == tool.sha256
            {
                tracing::info!(tool = tool.id, path = %path.display(), "工具缓存命中");
                return Ok(path);
            }
            tracing::warn!(tool = tool.id, "工具缓存摘要不符，删除后重下");
            let _ = tokio::fs::remove_file(&path).await;
        }
        self.download(tool, &path).await?;
        Ok(path)
    }

    async fn download(&self, tool: &ToolEntry, path: &Path) -> CoreResult<()> {
        tokio::fs::create_dir_all(&self.cache_dir)
            .await
            .map_err(|error| {
                CoreError::Internal(format!(
                    "创建工具缓存目录 {} 失败: {error}",
                    self.cache_dir.display()
                ))
            })?;
        // 先写临时名再 rename：中途失败不会留下半截文件被下次当成"缓存命中"。
        let tmp = path.with_extension("part");
        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent(concat!("app-reverse-tools/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| CoreError::Internal(format!("创建下载客户端失败: {error}")))?;
        let response = client
            .get(tool.download_url)
            .send()
            .await
            .map_err(|error| CoreError::Internal(format!("下载 {} 失败: {error}", tool.id)))?;
        if !response.status().is_success() {
            return Err(CoreError::Internal(format!(
                "下载 {} 失败: HTTP {}",
                tool.id,
                response.status()
            )));
        }
        if let Some(length) = response.content_length()
            && length > MAX_DOWNLOAD_BYTES
        {
            return Err(CoreError::Internal(format!(
                "{} 的下载体积异常（{length} 字节），已中止",
                tool.id
            )));
        }
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|error| CoreError::Internal(format!("写入 {} 失败: {error}", tmp.display())))?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut stream = response;
        use tokio::io::AsyncWriteExt;
        loop {
            let chunk = stream
                .chunk()
                .await
                .map_err(|error| CoreError::Internal(format!("下载 {} 中断: {error}", tool.id)))?;
            let Some(chunk) = chunk else { break };
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(CoreError::Internal(format!(
                    "{} 超过单次下载上限，已中止",
                    tool.id
                )));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|error| {
                CoreError::Internal(format!("写入 {} 失败: {error}", tmp.display()))
            })?;
        }
        file.flush().await.ok();
        drop(file);

        let digest = hex(&hasher.finalize());
        // 两件事都必须对上：体积（GitHub 对同名资源重发时最先变的就是它）与摘要。
        // 只比摘要也行，但体积不同时能给出一句更具体的话——"下到 8.5 MB 就断了"。
        if written != tool.size_bytes {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CoreError::Internal(format!(
                "{} 下载不完整：期望 {} 字节，实际 {} 字节",
                tool.id, tool.size_bytes, written
            )));
        }
        if digest != tool.sha256 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(CoreError::Internal(format!(
                "{} 校验失败：摘要 {} 与目录登记的 {} 不一致，已丢弃（上游可能替换了 release 资产）",
                tool.id, digest, tool.sha256
            )));
        }
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|error| CoreError::Internal(format!("落盘 {} 失败: {error}", path.display())))?;
        tracing::info!(tool = tool.id, bytes = written, %digest, "工具下载完成");
        Ok(())
    }

    /// 一键安装：本机下载（或命中缓存）→ 推送 → `chmod +x` → 回读复核。
    ///
    /// 回读复核（`ls -l` 字节数 + `test -x`）不是自证清白，而是"adb push 报了成功
    /// 但文件不完整"这类事的唯一可判据处；对不上就如实报 `verified_bytes`。
    pub async fn install(&self, serial: &str, id: &str) -> CoreResult<ToolInstallReport> {
        let tool = self
            .entry(id)
            .ok_or_else(|| CoreError::NotFound(format!("工具市场里没有 {id}")))?;
        let local = self.ensure_local(tool).await?;

        let device_path = format!("{DEVICE_TMP}/{}", tool.device_file_name);
        let pushed = self
            .run_checked(
                serial,
                &adb::cmd_push(&local.to_string_lossy(), &device_path),
                PUSH_TIMEOUT,
                "工具推送",
            )
            .await?;
        tracing::info!(tool = tool.id, %device_path, stdout = %pushed.stdout.trim(), "工具已推送");

        // chmod +x 走 adb shell：/data/local/tmp 是 shell 自己可写的地方，不需要 su。
        let chmod = self
            .run_checked(
                serial,
                &adb::cmd_shell(&format!("chmod 0755 {device_path}")),
                PUSH_TIMEOUT,
                "设置执行权限",
            )
            .await?;
        if chmod.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "chmod 失败: {}",
                chmod.stderr.trim()
            )));
        }

        let (verified_bytes, executable) = self.verify(serial, &device_path).await;
        Ok(ToolInstallReport {
            id: tool.id.to_owned(),
            version: tool.version.to_owned(),
            serial: serial.to_owned(),
            device_path,
            bytes: tool.size_bytes,
            sha256: tool.sha256.to_owned(),
            verified_bytes,
            executable,
        })
    }

    /// 设备主 ABI（`ro.product.cpu.abi`）。拿不到返回 `None`，**不猜**：
    /// "未知"与"不兼容"是两件事，前端只有拿到明确的值才该拦安装。
    pub async fn device_abi(&self, serial: &str) -> Option<String> {
        let output = self
            .run_checked(
                serial,
                &adb::cmd_shell("getprop ro.product.cpu.abi"),
                PUSH_TIMEOUT,
                "读取设备 ABI",
            )
            .await
            .ok()?;
        let abi = output.stdout.trim().to_owned();
        (!abi.is_empty()).then_some(abi)
    }

    /// 设备侧复核：`stat -c %s` + `test -x`。两条都拿不到就返回 `(None, false)`，
    /// 不猜——"复核失败"与"没装上"是两件事，界面要分得开。
    async fn verify(&self, serial: &str, device_path: &str) -> (Option<u64>, bool) {
        let script = format!(
            "stat -c %s {device_path} 2>/dev/null; test -x {device_path} && echo EXEC || echo NOEXEC"
        );
        match self
            .run_checked(serial, &adb::cmd_shell(&script), PUSH_TIMEOUT, "复核工具")
            .await
        {
            Ok(output) => {
                let mut bytes = None;
                let mut executable = false;
                for line in output.stdout.lines().map(str::trim) {
                    if line == "EXEC" {
                        executable = true;
                    } else if line == "NOEXEC" {
                        executable = false;
                    } else if let Ok(size) = line.parse::<u64>() {
                        bytes = Some(size);
                    }
                }
                (bytes, executable)
            }
            Err(error) => {
                tracing::warn!(device_path, error = %error, "工具安装后的设备侧复核失败");
                (None, false)
            }
        }
    }

    async fn run_checked(
        &self,
        serial: &str,
        command: &[String],
        timeout: Duration,
        step: &'static str,
    ) -> CoreResult<AdbRunOutput> {
        let environment = self.runner.environment().await;
        let adb_path = environment
            .path
            .ok_or_else(|| CoreError::Internal(environment.hint.unwrap_or_else(|| "adb 不可用".into())))?;
        let arguments = adb::build_args(Some(serial), command);
        let output = self
            .runner
            .run(&adb_path, &arguments, timeout)
            .await
            .map_err(|error| CoreError::Internal(format!("{step}失败: {error}")))?;
        if output.exit_code != Some(0) {
            // 设备掉线要如实换成 DEVICE_OFFLINE，界面才谈得上切回在线的那台
            if let Some(hint) = adb::device_missing_hint(&output.stderr) {
                return Err(CoreError::DeviceOffline(hint));
            }
            return Err(CoreError::Internal(format!(
                "{step}失败: {}",
                output.stderr.trim()
            )));
        }
        Ok(output)
    }
}

async fn sha256_file(path: &Path) -> CoreResult<String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开 {} 失败: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| CoreError::Internal(format!("读取 {} 失败: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_is_installable_as_registered() {
        for tool in TOOLS {
            assert!(!tool.id.is_empty(), "id 不能为空");
            // 文件名是拼进设备路径的：任何分隔符/空白都会让路径指向别处，
            // 也会破坏 `chmod {path}` 这条不带引号的 shell 命令。
            assert!(
                tool.device_file_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
                "{} 的设备侧文件名不安全: {}",
                tool.id,
                tool.device_file_name
            );
            assert!(
                tool.download_url.starts_with("https://"),
                "{} 必须走 HTTPS: {}",
                tool.id,
                tool.download_url
            );
            assert_eq!(
                tool.sha256.len(),
                64,
                "{} 的摘要必须是 64 位 hex",
                tool.id
            );
            assert!(
                tool.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{} 的摘要里出现非 hex 字符",
                tool.id
            );
            assert!(tool.size_bytes > 0, "{} 的体积不能是 0", tool.id);
            assert!(
                tool.download_url.contains(tool.version),
                "{} 的下载地址必须钉在这个版本上，不能用 latest",
                tool.id
            );
            assert!(
                !tool.usage.iter().any(|line| line.contains('\n')),
                "{} 的用法示例必须是一行一条",
                tool.id
            );
        }
    }

    /// 缓存命中的判据是**摘要**，不是"文件存在"。这条钉住"同名文件被截断/被换掉之后
    /// 还会被当成缓存"这个静默故障——它推下去就是一个坏二进制，而界面上是成功。
    #[tokio::test]
    async fn cache_path_is_version_and_digest_scoped() {
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let service = ToolMarketService::new(runner, std::env::temp_dir().join("art-tools-test"));
        let tool = &TOOLS[0];
        let path = service.cache_path(tool);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with(&format!("{}-{}", tool.id, tool.version)),
            "缓存名必须带版本，否则升级后命中旧文件: {name}"
        );
        assert!(
            name.contains(&tool.sha256[..12]),
            "缓存名必须带摘要前缀，否则同名不同内容会被当成同一份: {name}"
        );
    }
}
