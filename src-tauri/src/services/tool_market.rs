//! 工具市场（P13）：内置工具目录 + 一键安装到设备。
//!
//! 边界（与 Zygisk 那层同源的纪律）：
//! - **目录默认是代码里的常量，不是网络上的清单**。每一条的版本、URL、大小、SHA-256 都写死，
//!   界面读到的就是我们要装的那个东西；不存在"服务端换了包，客户端不知道"这种情况。
//! - 下载只走 HTTPS，落盘前校验大小与摘要；校验不过就删掉，绝不把一个来路不明的
//!   ELF 推到设备上。
//! - 推送目标固定 `/data/local/tmp/`（adb shell 自己就能读写的地方），不写系统分区、
//!   不需要 root。装完 `chmod +x`，然后它就是一个普通命令行工具，交给二进制托管页去跑。
//! - 联网默认直连；用户在设置里填的代理（`app.net.proxy`）对这里**所有**请求生效，
//!   每次请求现读——TUN 模式不用填，没有 TUN 的用户填上 HTTP/SOCKS5 代理即可。
//!
//! **唯一的动态条目是 frida-server**（`version_source = "github-releases"`）：
//! server 必须与用户本机的 frida 客户端**同版本**，把版本钉死在代码里等于逼用户
//! 为了一个上游 bugfix 升级整个桌面端。它的版本列表实时取自 frida 官方 release，
//! 但"装下去的那个文件"仍然先算摘要、再与 GitHub 登记的资产摘要核对（老资产没有摘要时
//! 至少核对体积）——动态的是版本，不是校验。其余条目仍是固定登记。
//!
//! 为什么别的工具不这样：一个工具装哪个版本由我们验证过，升级是改一行常量 + 一次真机验证；
//! 让用户在不知情的情况下换掉设备上的二进制，收益远小于风险。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::adapters::adb;
use crate::core::error::{CoreError, CoreResult};
use crate::services::config_service::{ConfigService, KEY_NET_PROXY};
use crate::services::device_service::{AdbRunOutput, AdbRunner};

/// 设备侧目标目录：adb shell 可写、不需要 root、重启后仍在（`/data/local/tmp` 不清）。
pub const DEVICE_TMP: &str = "/data/local/tmp";
/// 下载超时：9 MB 在正常网络下几秒；给足 5 分钟是为了慢网与 GitHub 的跳转重定向。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// 推送超时：9 MB 走 USB 是秒级，留 3 分钟挡"adb 卡死"。
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);
/// 单次下载的硬上限：防止对端把 Content-Length 说成 1 GB 时把内存/磁盘吃光。
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// frida 官方 release 的 API 入口（按 tag 查单个 release 的资产信息）。
const FRIDA_RELEASES_URL: &str = "https://api.github.com/repos/frida/frida/releases";
/// frida 的全部 tag（**一次请求拿全**，只用它取版本号）。
///
/// 试过两种更"正统"的做法，都不如这一条：
/// 1. 翻 releases：一个 release 带一百来个资产，`per_page=100` 一页 30 MB，
///    翻到 16.x 要 70 MB——为下拉框拉这么多 JSON 不可接受；
/// 2. 翻 tags（`/tags?per_page=100`）：一页只有 40 KB，但要 2–3 次请求，
///    而这个网络下 `api.github.com` 的 TLS 握手会间歇性地被对端掐断，
///    请求次数直接等于失败概率。
///
/// `git/matching-refs/tags/` 不分页：849 个 tag 一次 274 KB 回来，本机再筛。
/// 资产信息仍然等用户选定了版本、真要点安装时按 tag 查一次（`resolve_frida_asset`）。
const FRIDA_TAGS_URL: &str = "https://api.github.com/repos/frida/frida/git/matching-refs/tags/";
/// 匿名 GitHub API 限额 60 次/小时：列表在内存里缓存 10 分钟，正常使用碰不到上限。
const FRIDA_RELEASES_TTL: Duration = Duration::from_secs(600);
/// 版本列表的请求超时（只取 JSON，比下载资产短得多）。
const FRIDA_API_TIMEOUT: Duration = Duration::from_secs(30);
/// 版本下限：只列 16.x 及以后（用户口径：从 16 开始）。
const FRIDA_MIN_MAJOR: u64 = 16;
/// frida-server 在目录里的固定 id（前端据此走"选版本 → 安装"那条路）。
pub const FRIDA_SERVER_ID: &str = "frida-server";

/// 目录里的一项工具（固定登记）。
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

/// 界面消费的目录项：固定登记与动态版本**同一种形状**。
///
/// 动态条目（frida-server）的 `version` / `download_url` / `size_bytes` / `sha256`
/// 为空或 0——它们要等用户在界面上选好版本、后端解析出资产之后才有值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketToolDto {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub version: String,
    pub download_url: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub device_file_name: String,
    pub device_abi: Vec<String>,
    pub usage: Vec<String>,
    /// 版本从哪来：`""` = 目录登记的固定版本；`"github-releases"` = 实时查上游 release
    pub version_source: String,
}

impl From<&ToolEntry> for MarketToolDto {
    fn from(tool: &ToolEntry) -> Self {
        Self {
            id: tool.id.to_owned(),
            name: tool.name.to_owned(),
            tagline: tool.tagline.to_owned(),
            description: tool.description.to_owned(),
            homepage: tool.homepage.to_owned(),
            license: tool.license.to_owned(),
            version: tool.version.to_owned(),
            download_url: tool.download_url.to_owned(),
            size_bytes: tool.size_bytes,
            sha256: tool.sha256.to_owned(),
            device_file_name: tool.device_file_name.to_owned(),
            device_abi: tool
                .device_abi
                .iter()
                .map(|abi| (*abi).to_owned())
                .collect(),
            usage: tool.usage.iter().map(|line| (*line).to_owned()).collect(),
            version_source: String::new(),
        }
    }
}

/// 内置目录（固定版本）。
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

/// frida-server 的目录项（版本动态）。
///
/// 只提供 Android arm64：其它架构的资产我们不做选择与校验，就不在界面上给假选项。
fn frida_server_entry() -> MarketToolDto {
    MarketToolDto {
        id: FRIDA_SERVER_ID.to_owned(),
        name: "frida-server".to_owned(),
        tagline: "动态注入框架的设备侧服务，版本跟随上游发布".to_owned(),
        description: "Frida 的设备侧注入服务：起在设备上之后，桌面端的 frida 客户端与本应用的\
会话都通过它注入目标进程。版本列表实时取自 frida 官方 release，**只提供 Android arm64**；\
装好到 /data/local/tmp/frida-server 后，到设备页以 root 启动/停止即可。\
server 版本需要与本机 frida 客户端一致（桌面端会在会话前置检查里核对）。"
            .to_owned(),
        homepage: "https://github.com/frida/frida/releases".to_owned(),
        // frida 的上游许可是 wxWindows Library Licence（LGPL 风格的弱 copyleft）
        license: "wxWindows Library Licence".to_owned(),
        version: String::new(),
        download_url: String::new(),
        size_bytes: 0,
        sha256: String::new(),
        device_file_name: "frida-server".to_owned(),
        device_abi: vec!["arm64-v8a".to_owned()],
        usage: vec![
            "su -c /data/local/tmp/frida-server &".to_owned(),
            "frida-ps -U".to_owned(),
            "frida -U -F -l hook.js".to_owned(),
        ],
        version_source: "github-releases".to_owned(),
    }
}

/// 下拉里的一个 frida 版本。
///
/// 只有版本号：体积 / 摘要 / 下载地址属于"某个版本的那个资产"，等用户选定了版本、
/// 真的要点安装时再按 tag 查一次（`resolve_frida_asset`）——列表不该为此拖 30 MB JSON。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FridaVersionDto {
    pub version: String,
}

/// 上游某个版本里、我们要下的那个资产（安装用，不出现在列表里）。
#[derive(Debug, Clone)]
struct FridaAsset {
    /// 纯数字版本（`17.24.3`），取自资产名本身
    version: String,
    asset_name: String,
    download_url: String,
    size_bytes: u64,
    /// GitHub 登记的资产摘要（sha256，小写 hex）；2025 年之前上传的老资产没有这一项
    sha256: Option<String>,
}

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

/// 网络失败时统一带的一句话：告诉用户在哪儿配代理（TUN 模式下不用配）。
const PROXY_HINT: &str = "；如果需要代理，去「设置 → 工具 → 代理地址」填好后重试（TUN 模式不用配）";

/// 联网探针的超时：只是"通不通"，不需要等太久。
const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(15);

pub struct ToolMarketService {
    runner: Arc<dyn AdbRunner>,
    /// 配置服务：每次发请求前现读代理设置（改完设置不用重启）
    config: Arc<ConfigService>,
    /// 下载缓存目录（本机 app data 下的 tools/），重复安装同一版本不必再下一次
    cache_dir: PathBuf,
    /// frida 版本列表的内存缓存：`(取到的时刻, 列表)`。GitHub 匿名限额下这是必需品。
    frida_cache: tokio::sync::Mutex<Option<(Instant, Vec<FridaVersionDto>)>>,
    /// 已经按 tag 解析过的资产（版本 → 资产）：同一版本重装不再问一次 GitHub。
    frida_assets: tokio::sync::Mutex<std::collections::HashMap<String, FridaAsset>>,
}

impl ToolMarketService {
    /// `cache_dir` 由调用方给（`lib.rs` 用 app data 目录）：这一层不需要知道
    /// ConfigService 的内部结构，测试也能直接指到临时目录。
    pub fn new(runner: Arc<dyn AdbRunner>, config: Arc<ConfigService>, cache_dir: PathBuf) -> Self {
        Self {
            runner,
            config,
            cache_dir,
            frida_cache: tokio::sync::Mutex::new(None),
            frida_assets: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// 当前配置的代理（空 = 直连）。每次请求现读：改完设置立刻生效。
    fn proxy_setting(&self) -> Option<String> {
        match self.config.get(KEY_NET_PROXY, "") {
            Ok(value) => {
                let value = value.trim().to_owned();
                (!value.is_empty()).then_some(value)
            }
            Err(error) => {
                tracing::warn!(error = %error, "读取代理设置失败，按直连处理");
                None
            }
        }
    }

    /// 统一的 HTTP 客户端：超时 + UA +（配置了就走）代理。
    ///
    /// 为什么每次请求都新建而不是缓存一个 Client：代理是用户可改的运行时设置，
    /// 缓存客户端会让"改完代理要重启应用"变成一条没人知道的暗坑。
    fn http_client(&self, timeout: Duration) -> CoreResult<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("app-reverse-tools/", env!("CARGO_PKG_VERSION")));
        if let Some(raw) = self.proxy_setting() {
            let url = normalize_proxy_url(&raw);
            let proxy = reqwest::Proxy::all(&url).map_err(|error| {
                CoreError::InvalidInput(format!(
                    "代理地址不合法：{raw}（{error}）——去「设置 → 工具 → 代理地址」改一下"
                ))
            })?;
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|error| CoreError::Internal(format!("创建 HTTP 客户端失败: {error}")))
    }

    /// 下载前的连通性检查：**不通就把"去配代理"说在前面**，而不是让用户等一次
    /// 注定失败的 20+ MB 下载。探的是 github.com 首页——它不是 GitHub API，不占配额。
    async fn ensure_network_reachable(&self, step: &str) -> CoreResult<()> {
        let client = self.http_client(REACHABILITY_TIMEOUT)?;
        match client.get("https://github.com/").send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.is_redirection() {
                    Ok(())
                } else {
                    Err(CoreError::Internal(format!(
                        "{step}：GitHub 返回 HTTP {status}{PROXY_HINT}"
                    )))
                }
            }
            Err(error) => Err(CoreError::Internal(format!(
                "{step}：连不上 GitHub（{}）{PROXY_HINT}",
                error_chain(&error)
            ))),
        }
    }

    /// 界面目录 = 固定登记的工具 + 动态版本的 frida-server。
    pub fn catalog(&self) -> Vec<MarketToolDto> {
        let mut tools: Vec<MarketToolDto> = TOOLS.iter().map(MarketToolDto::from).collect();
        tools.push(frida_server_entry());
        tools
    }

    /// 固定登记的条目（安装路径只认这些；frida-server 走 `install_frida`）。
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
        // 只在**真的要下载**时做连通性检查：缓存命中时离线也能装
        self.ensure_network_reachable(&format!("下载 {}", tool.id))
            .await?;
        let tmp = part_path(path);
        let (written, digest) = self
            .download_stream(tool.download_url, tool.id, &tmp)
            .await?;
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
        tokio::fs::rename(&tmp, path).await.map_err(|error| {
            CoreError::Internal(format!("落盘 {} 失败: {error}", path.display()))
        })?;
        tracing::info!(tool = tool.id, bytes = written, %digest, "工具下载完成");
        Ok(())
    }

    /// 流式下载到 `tmp`（自动建父目录），返回 `(字节数, SHA-256)`。
    ///
    /// 不在这里判"期望体积/摘要"：固定条目与动态条目的判据不同，由各自的调用点判，
    /// 这样错误话术能说清是"下载断了"还是"摘要对不上"。
    async fn download_stream(
        &self,
        url: &str,
        label: &str,
        tmp: &Path,
    ) -> CoreResult<(u64, String)> {
        if let Some(parent) = tmp.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                CoreError::Internal(format!("创建缓存目录 {} 失败: {error}", parent.display()))
            })?;
        }
        let client = self.http_client(DOWNLOAD_TIMEOUT)?;
        let response = client.get(url).send().await.map_err(|error| {
            CoreError::Internal(format!(
                "下载 {label} 失败: {}{PROXY_HINT}",
                error_chain(&error)
            ))
        })?;
        if !response.status().is_success() {
            return Err(CoreError::Internal(format!(
                "下载 {label} 失败: HTTP {}",
                response.status()
            )));
        }
        if let Some(length) = response.content_length()
            && length > MAX_DOWNLOAD_BYTES
        {
            return Err(CoreError::Internal(format!(
                "{label} 的下载体积异常（{length} 字节），已中止"
            )));
        }
        let mut file = tokio::fs::File::create(tmp).await.map_err(|error| {
            CoreError::Internal(format!("写入 {} 失败: {error}", tmp.display()))
        })?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut stream = response;
        use tokio::io::AsyncWriteExt;
        loop {
            let chunk = stream
                .chunk()
                .await
                .map_err(|error| CoreError::Internal(format!("下载 {label} 中断: {error}")))?;
            let Some(chunk) = chunk else { break };
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                let _ = tokio::fs::remove_file(tmp).await;
                return Err(CoreError::Internal(format!(
                    "{label} 超过单次下载上限，已中止"
                )));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|error| {
                CoreError::Internal(format!("写入 {} 失败: {error}", tmp.display()))
            })?;
        }
        file.flush().await.ok();
        drop(file);
        Ok((written, hex(&hasher.finalize())))
    }

    // ===== frida-server（动态版本）=====

    /// 实时取 frida 的版本列表（16.x 及以后，按版本从新到旧）。
    ///
    /// 一次请求拿全部 tag（见 `FRIDA_TAGS_URL` 处的取舍说明），本机筛出 16+ 再排序。
    /// `refresh = true` 跳过内存缓存（界面上的「重新获取」）。缓存只有 10 分钟：
    /// GitHub 匿名限额 60 次/小时，而"刚发布的版本"本来就以小时计地晚于开界面。
    pub async fn frida_releases(&self, refresh: bool) -> CoreResult<Vec<FridaVersionDto>> {
        if !refresh {
            let cached = self.frida_cache.lock().await;
            if let Some((at, list)) = cached.as_ref()
                && at.elapsed() < FRIDA_RELEASES_TTL
            {
                return Ok(list.clone());
            }
        }

        let value = self
            .github_get(FRIDA_TAGS_URL, "获取 frida 版本列表")
            .await?;
        let items = value.as_array().cloned().unwrap_or_default();
        let mut versions = parse_frida_refs(&items);
        versions.sort_by_key(|version| std::cmp::Reverse(release_key(version)));
        versions.dedup();
        if versions.is_empty() {
            return Err(CoreError::NotFound(
                "frida 官方 tags 里没有 16.x 及以后的版本标签（上游改了命名？）".into(),
            ));
        }
        let list: Vec<FridaVersionDto> = versions
            .into_iter()
            .map(|version| FridaVersionDto { version })
            .collect();
        tracing::info!(
            count = list.len(),
            latest = %list[0].version,
            oldest = %list[list.len() - 1].version,
            "frida 版本列表已更新"
        );
        *self.frida_cache.lock().await = Some((Instant::now(), list.clone()));
        Ok(list)
    }

    /// 把一个版本解析成"要下的那个资产"（`releases/tags/{version}`，一次一个 release）。
    ///
    /// 解析结果留在内存里：同一版本重装（或先看版本、再装）不会重复问 GitHub。
    async fn resolve_frida_asset(&self, version: &str) -> CoreResult<FridaAsset> {
        validate_frida_version(version)?;
        if let Some(found) = self.frida_assets.lock().await.get(version).cloned() {
            return Ok(found);
        }
        let url = format!("{FRIDA_RELEASES_URL}/tags/{version}");
        let value = self.github_get(&url, "获取 frida release").await?;
        let asset = parse_release(&value).ok_or_else(|| {
            CoreError::NotFound(format!(
                "frida {version} 没有 android-arm64 的 frida-server 资产"
            ))
        })?;
        if asset.version != version {
            return Err(CoreError::Internal(format!(
                "frida {version} 的资产版本对不上（拿到 {}），已中止",
                asset.version
            )));
        }
        self.frida_assets
            .lock()
            .await
            .insert(version.to_owned(), asset.clone());
        Ok(asset)
    }

    /// 确保本机有一份解压好的 frida-server，返回 `(路径, 字节数, SHA-256)`。
    ///
    /// 归档（.xz）是缓存单位：它在 HTTPS 下下载，且与 GitHub 登记的资产摘要核对过；
    /// 每次安装都**重新解压**（几秒钟）而不是缓存解压结果——解压是确定性的，
    /// 少一个"解压出来的文件被截断/替换后还当缓存用"的静默故障点。
    async fn ensure_frida_binary(&self, asset: &FridaAsset) -> CoreResult<(PathBuf, u64, String)> {
        let archive = self.cache_dir.join(format!(
            "frida-server-{}-android-arm64-{}.xz",
            asset.version, asset.size_bytes
        ));

        let mut reuse = match tokio::fs::metadata(&archive).await {
            Ok(meta) => meta.len() == asset.size_bytes,
            Err(_) => false,
        };
        if reuse {
            if let Some(expected) = &asset.sha256 {
                reuse = sha256_file(&archive)
                    .await
                    .map(|actual| actual == *expected)
                    .unwrap_or(false);
            }
            if !reuse {
                tracing::warn!(asset = %asset.asset_name, "frida-server 归档缓存不符，删除后重下");
                let _ = tokio::fs::remove_file(&archive).await;
            }
        }
        if !reuse {
            // 只在真要下载时检查连通性（缓存有效时离线也能装）
            self.ensure_network_reachable("下载 frida-server").await?;
            let tmp = part_path(&archive);
            let (written, digest) = self
                .download_stream(&asset.download_url, &asset.asset_name, &tmp)
                .await?;
            if written != asset.size_bytes {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(CoreError::Internal(format!(
                    "{} 下载不完整：期望 {} 字节，实际 {} 字节",
                    asset.asset_name, asset.size_bytes, written
                )));
            }
            if let Some(expected) = &asset.sha256
                && digest != *expected
            {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(CoreError::Internal(format!(
                    "{} 校验失败：摘要 {} 与 GitHub 登记的 {} 不一致，已丢弃",
                    asset.asset_name, digest, expected
                )));
            }
            tokio::fs::rename(&tmp, &archive).await.map_err(|error| {
                CoreError::Internal(format!("落盘 {} 失败: {error}", archive.display()))
            })?;
            tracing::info!(asset = %asset.asset_name, bytes = written, %digest, "frida-server 归档下载完成");
        }

        let binary = self
            .cache_dir
            .join(format!("frida-server-{}-android-arm64", asset.version));
        let (archive_for_task, binary_for_task) = (archive.clone(), binary.clone());
        let bytes =
            tokio::task::spawn_blocking(move || extract_xz(&archive_for_task, &binary_for_task))
                .await
                .map_err(|error| CoreError::Internal(format!("解压任务被中断：{error}")))??;
        let sha256 = sha256_file(&binary).await?;
        Ok((binary, bytes, sha256))
    }

    /// 安装 frida-server：解析资产 →（需要下载时先做连通性检查）下载 + 摘要校验 →
    /// 本机解压 → 推送 → `chmod +x` → 回读复核。
    ///
    /// 设备侧文件固定叫 `frida-server`：设备页的启动/停止、Agent 的默认二进制名都按它走。
    /// 只支持 Android arm64——设备明确报了别的 ABI 就拦；读不到 ABI 视为未知（不拦，
    /// 与界面同一条口径）。
    pub async fn install_frida(
        &self,
        serial: &str,
        version: &str,
    ) -> CoreResult<ToolInstallReport> {
        validate_frida_version(version)?;
        if let Some(abi) = self.device_abi(serial).await
            && !abi.to_ascii_lowercase().starts_with("arm64")
        {
            return Err(CoreError::InvalidInput(format!(
                "frida-server 目前只提供 android-arm64：这台设备是 {abi}"
            )));
        }
        let asset = self.resolve_frida_asset(version).await?;
        let (local, bytes, sha256) = self.ensure_frida_binary(&asset).await?;
        self.push_and_verify(
            serial,
            &local,
            FRIDA_SERVER_ID,
            version,
            "frida-server",
            bytes,
            sha256,
        )
        .await
    }

    /// GitHub API 的 GET（只取 JSON）。错误话术要能区分"限流 / 404 / 网络"。
    ///
    /// **传输层错误重试两次**：实测这台机器走透明代理时，`api.github.com` 的 TLS 握手
    /// 会间歇性被对端掐断（8 次里 2 次 `tls handshake eof`），而 curl / 浏览器同刻是通的。
    /// 重试把一次抖动变成一次慢一点的成功；限流（403/429）与 404 **不重试**——
    /// 重试只会再花一次配额、得到同一句话。
    async fn github_get(&self, url: &str, label: &str) -> CoreResult<Value> {
        const ATTEMPTS: u32 = 3;
        let mut last_error = None;
        for attempt in 1..=ATTEMPTS {
            match self.github_get_once(url, label).await {
                Ok(value) => return Ok(value),
                Err((error, retryable)) => {
                    tracing::warn!(attempt, label, error = %error, retryable, "GitHub 请求失败");
                    last_error = Some(error);
                    if !retryable || attempt == ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
                }
            }
        }
        Err(last_error.expect("循环至少跑了一次，失败时一定有错误"))
    }

    /// 单次 GET。`Err((错误, 是否值得重试))`。
    async fn github_get_once(&self, url: &str, label: &str) -> Result<Value, (CoreError, bool)> {
        let client = self
            .http_client(FRIDA_API_TIMEOUT)
            .map_err(|error| (error, false))?;
        let response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| {
                (
                    CoreError::Internal(format!(
                        "{label}失败: {}{PROXY_HINT}",
                        error_chain(&error)
                    )),
                    true,
                )
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            (
                CoreError::Internal(format!("{label}失败: 读取响应出错: {error}")),
                true,
            )
        })?;
        if !status.is_success() {
            // GitHub 的错误 JSON 带 message；限流还额外说明一下配额，免得用户以为是网络坏了
            let message = serde_json::from_str::<Value>(&body).ok().and_then(|value| {
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            let detail = message.map(|m| format!(" · {m}")).unwrap_or_default();
            if status.as_u16() == 404 {
                return Err((
                    CoreError::NotFound(format!("{label}：上游没有这个资源（HTTP 404）")),
                    false,
                ));
            }
            let rate_limited = status.as_u16() == 403 || status.as_u16() == 429;
            let hint = if rate_limited {
                "（GitHub 匿名请求每小时 60 次，稍后再试）"
            } else {
                ""
            };
            return Err((
                CoreError::Internal(format!("{label}失败: HTTP {status}{detail}{hint}")),
                // 5xx 值得重试；限流与其它 4xx 不值得
                status.is_server_error(),
            ));
        }
        serde_json::from_str(&body).map_err(|error| {
            (
                CoreError::Internal(format!("{label}返回的不是 JSON: {error}")),
                true,
            )
        })
    }

    /// 一键安装（固定版本工具）：本机下载（需要下载时先做连通性检查；缓存命中则离线可装）
    /// → 推送 → `chmod +x` → 回读复核。
    pub async fn install(&self, serial: &str, id: &str) -> CoreResult<ToolInstallReport> {
        if id == FRIDA_SERVER_ID {
            return Err(CoreError::InvalidInput(
                "frida-server 要指定版本：走版本安装入口".into(),
            ));
        }
        let tool = self
            .entry(id)
            .ok_or_else(|| CoreError::NotFound(format!("工具市场里没有 {id}")))?;
        let local = self.ensure_local(tool).await?;
        self.push_and_verify(
            serial,
            &local,
            tool.id,
            tool.version,
            tool.device_file_name,
            tool.size_bytes,
            tool.sha256.to_owned(),
        )
        .await
    }

    /// 推送 + `chmod +x` + 回读复核（固定条目与 frida-server 共用）。
    ///
    /// 回读复核（`stat -c %s` 字节数 + `test -x`）不是自证清白，而是"adb push 报了成功
    /// 但文件不完整"这类事的唯一可判据处；对不上就如实报 `verified_bytes`。
    #[allow(clippy::too_many_arguments)]
    async fn push_and_verify(
        &self,
        serial: &str,
        local: &Path,
        id: &str,
        version: &str,
        device_file_name: &str,
        bytes: u64,
        sha256: String,
    ) -> CoreResult<ToolInstallReport> {
        let device_path = format!("{DEVICE_TMP}/{device_file_name}");
        let pushed = self
            .run_checked(
                serial,
                &adb::cmd_push(&local.to_string_lossy(), &device_path),
                PUSH_TIMEOUT,
                "工具推送",
            )
            .await?;
        tracing::info!(tool = id, %device_path, stdout = %pushed.stdout.trim(), "工具已推送");

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
            id: id.to_owned(),
            version: version.to_owned(),
            serial: serial.to_owned(),
            device_path,
            bytes,
            sha256,
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
        let adb_path = environment.path.ok_or_else(|| {
            CoreError::Internal(environment.hint.unwrap_or_else(|| "adb 不可用".into()))
        })?;
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

/// 代理地址归一：省略 scheme 的 `host:port` 按 http 处理（设置页的输入可以很随意）。
fn normalize_proxy_url(raw: &str) -> String {
    let value = raw.trim();
    if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    }
}

/// `xyz.xz` → `xyz.xz.part`。
///
/// 不能用 `Path::set_extension`：frida-server 的文件名里带版本点号
/// （`frida-server-17.24.3-android-arm64`），`set_extension` 会把 `3-android-arm64`
/// 当成扩展名一起换掉，落出一个叫 `frida-server-17.24.part` 的东西。
fn part_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    path.with_file_name(name)
}

/// frida-server 的资产名：`frida-server-<版本>-android-arm64.xz`（只认 arm64）。
fn frida_asset_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^frida-server-(\d+\.\d+\.\d+)-android-arm64\.xz$").expect("写死的正则必须有效")
    })
}

/// GitHub 的资产摘要形如 `sha256:<64 hex>`；老资产没有这一项。
fn parse_sha256_digest(raw: &str) -> Option<String> {
    let hex = raw
        .strip_prefix("sha256:")
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

/// 单条 release → 我们要的那个资产；不是稳定版 / 没有 arm64 资产就 `None`。
fn parse_release(value: &Value) -> Option<FridaAsset> {
    if value.get("draft").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    if value
        .get("prerelease")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let assets = value.get("assets")?.as_array()?;
    let mut found = None;
    for asset in assets {
        let Some(name) = asset.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(captures) = frida_asset_regex().captures(name) else {
            continue;
        };
        let Some(version) = captures.get(1).map(|m| m.as_str().to_owned()) else {
            continue;
        };
        let size_bytes = asset.get("size").and_then(Value::as_u64).unwrap_or(0);
        if size_bytes == 0 {
            continue;
        }
        let Some(download_url) = asset
            .get("browser_download_url")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("https://"))
        else {
            continue;
        };
        found = Some(FridaAsset {
            version,
            asset_name: name.to_owned(),
            download_url: download_url.to_owned(),
            size_bytes,
            sha256: asset
                .get("digest")
                .and_then(Value::as_str)
                .and_then(parse_sha256_digest),
        });
        // 一个 release 里只该有一个 frida-server android-arm64 资产
        break;
    }
    found
}

/// `git/matching-refs/tags/` 的响应 → 可用的版本号（16.x 及以后）。
///
/// 每条形如 `{"ref": "refs/tags/17.24.3", …}`；`17.17.1-barebone.N` 这类非发布标签、
/// 以及 15 及更老的标签都跳过。排序由调用方做（上游给的是字典序，不可用）。
fn parse_frida_refs(items: &[Value]) -> Vec<String> {
    let mut versions = Vec::new();
    for item in items {
        let Some(reference) = item.get("ref").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        if !is_numeric_version(name) || release_key(name).0 < FRIDA_MIN_MAJOR {
            continue;
        }
        versions.push(name.to_owned());
    }
    versions
}

/// 严格的三段纯数字版本（与 `validate_frida_version` 同一口径）。
fn is_numeric_version(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// 版本号的比较键。只用于排序（上游一定给三段数字，解析失败就排到最后）。
fn release_key(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split('.').filter_map(|p| p.parse::<u64>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// 版本号只能是我们认的 `X.Y.Z`：它会被拼进 URL 与文件名，不接受任何别的东西。
fn validate_frida_version(version: &str) -> CoreResult<()> {
    if is_numeric_version(version) {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(format!(
            "frida-server 版本号不合法（要 X.Y.Z）：{version}"
        )))
    }
}

/// 解压 `.xz` 到 `out`（先写 `.part` 再 rename：解压中断不会留下半截文件）。
/// 同步阻塞（CPU 密集），调用方要放在 `spawn_blocking` 里。
fn extract_xz(archive: &Path, out: &Path) -> CoreResult<u64> {
    use std::io::Write;

    let input = std::fs::File::open(archive).map_err(|error| {
        CoreError::Internal(format!("打开 {} 失败: {error}", archive.display()))
    })?;
    let mut reader = std::io::BufReader::new(input);
    let tmp = part_path(out);
    let file = std::fs::File::create(&tmp)
        .map_err(|error| CoreError::Internal(format!("写入 {} 失败: {error}", tmp.display())))?;
    let mut writer = std::io::BufWriter::new(file);
    if let Err(error) = lzma_rs::xz_decompress(&mut reader, &mut writer) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Internal(format!(
            "解压 {} 失败: {error}",
            archive.display()
        )));
    }
    if let Err(error) = writer.flush() {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Internal(format!(
            "写入 {} 失败: {error}",
            tmp.display()
        )));
    }
    drop(writer);

    let bytes = std::fs::metadata(&tmp).map(|meta| meta.len()).unwrap_or(0);
    if bytes == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Internal(format!(
            "解压 {} 得到空文件",
            archive.display()
        )));
    }
    std::fs::rename(&tmp, out)
        .map_err(|error| CoreError::Internal(format!("落盘 {} 失败: {error}", out.display())))?;
    Ok(bytes)
}

async fn sha256_file(path: &Path) -> CoreResult<String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开 {} 失败: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            CoreError::Internal(format!("读取 {} 失败: {error}", path.display()))
        })?;
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

/// reqwest 的 `Display` 往往只有一句 "error sending request for url (…)"，
/// 真正的原因（DNS / TLS 握手 / 连接被重置）在 source 链里。界面要给用户看的是后者。
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        out.push_str(&format!(" → {cause}"));
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的配置服务（内存库）：代理默认空 = 直连。
    fn test_config() -> Arc<ConfigService> {
        Arc::new(ConfigService::new(Arc::new(
            crate::db::Db::in_memory().unwrap(),
        )))
    }

    /// 代理地址归一：设置页允许只写 `host:port`，按 http 处理；带 scheme 的原样。
    #[test]
    fn proxy_url_without_scheme_defaults_to_http() {
        assert_eq!(
            normalize_proxy_url(" 127.0.0.1:7890 "),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:7891"),
            "socks5://127.0.0.1:7891"
        );
    }

    /// 配置了代理之后，请求必须真的走代理：用一个本地假代理接住 CONNECT，
    /// 然后回 502——不需要外网也能判定"走没走代理"。
    #[tokio::test]
    async fn configured_proxy_receives_the_connect_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("起本地假代理");
        let port = listener.local_addr().expect("端口").port();
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let seen_in_task = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0_u8; 2048];
                if let Ok(read) = socket.read(&mut buffer).await {
                    seen_in_task
                        .lock()
                        .await
                        .push(String::from_utf8_lossy(&buffer[..read]).to_string());
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });

        let config = test_config();
        // 故意不带 scheme：顺带钉住归一化（http:// 由后端补）
        config
            .set(KEY_NET_PROXY, &format!("127.0.0.1:{port}"))
            .expect("写代理设置");
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let dir = tempfile::tempdir().expect("临时目录");
        let service = ToolMarketService::new(runner, config, dir.path().to_path_buf());

        let error = service
            .ensure_network_reachable("测试下载前检查")
            .await
            .expect_err("假代理回 502，连通性检查必须失败");
        assert!(
            error.to_string().contains("如果需要代理"),
            "失败时要给出「去配代理」的方向: {error}"
        );
        let requests = seen.lock().await;
        assert!(
            requests
                .iter()
                .any(|request| request.starts_with("CONNECT github.com:443")),
            "请求必须走代理（收到 CONNECT 才算）: {:?}",
            *requests
        );
    }

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
            assert_eq!(tool.sha256.len(), 64, "{} 的摘要必须是 64 位 hex", tool.id);
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
        let service = ToolMarketService::new(
            runner,
            test_config(),
            std::env::temp_dir().join("art-tools-test"),
        );
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

    #[test]
    fn catalog_adds_frida_server_as_a_dynamic_entry() {
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let service = ToolMarketService::new(
            runner,
            test_config(),
            std::env::temp_dir().join("art-tools-test"),
        );
        let catalog = service.catalog();
        let frida = catalog
            .iter()
            .find(|tool| tool.id == FRIDA_SERVER_ID)
            .expect("目录里必须有 frida-server");
        assert_eq!(frida.version_source, "github-releases");
        assert!(frida.version.is_empty(), "动态条目的版本不能写死");
        assert!(frida.download_url.is_empty(), "动态条目没有固定下载地址");
        assert_eq!(frida.device_file_name, "frida-server");
        assert_eq!(frida.device_abi, vec!["arm64-v8a".to_owned()]);
        // 固定条目不受影响
        let stackplz = catalog
            .iter()
            .find(|tool| tool.id == "stackplz")
            .expect("目录里仍有 stackplz");
        assert!(stackplz.version_source.is_empty());
        assert!(!stackplz.version.is_empty());
    }

    fn release(tag: &str, asset: Option<(&str, u64, Option<&str>)>, prerelease: bool) -> Value {
        let assets = match asset {
            Some((name, size, digest)) => serde_json::json!([{
                "name": name,
                "size": size,
                "browser_download_url": format!("https://github.com/frida/frida/releases/download/{tag}/{name}"),
                "digest": digest,
            }]),
            None => serde_json::json!([]),
        };
        serde_json::json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": prerelease,
            "published_at": "2026-10-27T00:00:00Z",
            "assets": assets,
        })
    }

    /// 资产解析：只认 `frida-server-<X.Y.Z>-android-arm64.xz`，稳定版，摘要照抄 GitHub 的。
    #[test]
    fn parse_release_finds_the_arm64_asset() {
        let value = release(
            "17.24.2",
            Some((
                "frida-server-17.24.2-android-arm64.xz",
                24_627_616,
                Some("sha256:e746ffa89d328c65dc9cf05ae98f08dd0e0571df7e51889033e8bed07f2cc3ea"),
            )),
            false,
        );
        let asset = parse_release(&value).expect("稳定版应能解析出资产");
        assert_eq!(asset.version, "17.24.2");
        assert_eq!(asset.size_bytes, 24_627_616);
        assert_eq!(
            asset.sha256.as_deref(),
            Some("e746ffa89d328c65dc9cf05ae98f08dd0e0571df7e51889033e8bed07f2cc3ea")
        );
        assert_eq!(
            asset.download_url,
            "https://github.com/frida/frida/releases/download/17.24.2/frida-server-17.24.2-android-arm64.xz"
        );

        // prerelease / 没有 arm64 资产 / 没有资产：都不认
        assert!(
            parse_release(&release(
                "18.0.0",
                Some(("frida-server-18.0.0-android-arm64.xz", 1, None)),
                true
            ))
            .is_none()
        );
        assert!(
            parse_release(&release(
                "17.24.1",
                Some(("frida-server-17.24.1-android-x86_64.xz", 2, None)),
                false
            ))
            .is_none()
        );
        assert!(parse_release(&release("17.24.0", None, false)).is_none());
    }

    /// refs 解析规则：严格 X.Y.Z、只留 16 及以后、跳过 barebone 之类的非发布标签。
    #[test]
    fn parse_refs_filters_non_versions_and_old_majors() {
        let items: Vec<Value> = [
            "refs/tags/1.0.0",
            "refs/tags/15.1.9",
            "refs/tags/16.0.0",
            "refs/tags/16.0.7",
            "refs/tags/17.2.2",
            "refs/tags/17.17.1-barebone.18",
            "refs/tags/17.24.3",
            "refs/tags/17.24.10",
        ]
        .into_iter()
        .map(|reference| serde_json::json!({ "ref": reference }))
        .collect();
        let mut versions = parse_frida_refs(&items);
        versions.sort_by(|a, b| release_key(b).cmp(&release_key(a)));
        assert_eq!(
            versions,
            vec!["17.24.10", "17.24.3", "17.2.2", "16.0.7", "16.0.0"],
            "只留 16 及以后的三段数字版本（上游给的是字典序，排序归调用方）"
        );
    }

    #[test]
    fn version_validation_blocks_path_and_aliases() {
        for good in ["17.24.3", "16.0.1", "1.2.3"] {
            assert!(validate_frida_version(good).is_ok(), "{good} 应被接受");
        }
        for bad in [
            "latest",
            "17.24",
            "v17.24.3",
            "17.24.3-rc1",
            "../17.24.3",
            "17..3",
            "",
        ] {
            assert!(validate_frida_version(bad).is_err(), "{bad} 必须被拒绝");
        }
    }

    /// 解压走的是真 .xz（fixture 里是 `send("hello from xz");\n`）：
    /// 只测"函数被调用"没有意义，要测出来的字节就是压进去的那些。
    #[test]
    fn extract_xz_writes_decompressed_bytes() {
        let dir = tempfile::tempdir().expect("临时目录");
        let archive = dir.path().join("mini.xz");
        std::fs::write(&archive, include_bytes!("fixtures/frida-mini.js.xz")).expect("写 fixture");
        let out = dir.path().join("frida-server-17.24.3-android-arm64");
        let bytes = extract_xz(&archive, &out).expect("解压应成功");
        assert_eq!(bytes, std::fs::metadata(&out).unwrap().len());
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "send(\"hello from xz\");\n"
        );
        // `.part` 是过程文件，成功后不该留下
        assert!(!part_path(&out).exists());
    }

    #[test]
    fn extract_xz_rejects_garbage_without_leaving_part_files() {
        let dir = tempfile::tempdir().expect("临时目录");
        let archive = dir.path().join("bad.xz");
        std::fs::write(&archive, b"not an xz archive").expect("写假归档");
        let out = dir.path().join("frida-server-17.24.3-android-arm64");
        assert!(extract_xz(&archive, &out).is_err());
        assert!(!out.exists());
        assert!(!part_path(&out).exists(), "失败时不许留下半截文件");
    }

    /// 版本点号不能被 `set_extension` 当成扩展名吃掉（真实事故形状）。
    #[test]
    fn part_path_keeps_version_dots() {
        let path = Path::new("/tmp/frida-server-17.24.3-android-arm64");
        assert_eq!(
            part_path(path),
            Path::new("/tmp/frida-server-17.24.3-android-arm64.part")
        );
        let archive = Path::new("/tmp/frida-server-17.24.3-android-arm64-24624060.xz");
        assert_eq!(
            part_path(archive),
            Path::new("/tmp/frida-server-17.24.3-android-arm64-24624060.xz.part")
        );
    }

    /// 固定版本的安装入口不许碰动态条目：frida-server 必须带上用户选的版本。
    #[tokio::test]
    async fn fixed_install_rejects_dynamic_frida_id() {
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let service = ToolMarketService::new(
            runner,
            test_config(),
            std::env::temp_dir().join("art-tools-test"),
        );
        let error = service
            .install("SERIAL", FRIDA_SERVER_ID)
            .await
            .expect_err("frida-server 不该走固定安装入口");
        assert!(
            error.to_string().contains("版本安装入口"),
            "错误要说清该走哪条路: {error}"
        );
    }

    #[tokio::test]
    #[ignore = "联网：手工跑一次确认版本列表、资产解析、下载、校验、解压"]
    async fn live_frida_release_download_and_extract() {
        let runner: Arc<dyn AdbRunner> =
            Arc::new(crate::services::device_service::MockAdbRunner::new(true));
        let dir = tempfile::tempdir().expect("临时目录");
        let service = ToolMarketService::new(runner, test_config(), dir.path().to_path_buf());
        let list = service.frida_releases(true).await.expect("取版本列表");
        assert!(!list.is_empty(), "上游至少要有版本");
        assert!(
            list.iter().any(|v| v.version.starts_with("16.")),
            "列表必须一直列到 16.x，实际末位 {:?}",
            list.last().map(|v| &v.version)
        );
        println!(
            "versions: {} ({} .. {})",
            list.len(),
            list[0].version,
            list[list.len() - 1].version
        );
        let version = list[0].version.clone();
        let asset = service
            .resolve_frida_asset(&version)
            .await
            .expect("解析资产");
        let (path, bytes, sha256) = service
            .ensure_frida_binary(&asset)
            .await
            .expect("下载并解压");
        println!("{} -> {} bytes, sha256={sha256}", path.display(), bytes);
        assert!(bytes > 10_000_000, "解压出来的 frida-server 太小: {bytes}");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("frida-server-{version}-android-arm64")
        );
    }
}
