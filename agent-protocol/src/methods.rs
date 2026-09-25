use serde::{Deserialize, Serialize};

use crate::CapabilityInfo;

pub mod method {
    pub const SYSTEM_HELLO: &str = "system.hello";
    pub const SYSTEM_HEALTH: &str = "system.health";
    pub const CAPABILITY_LIST: &str = "capability.list";
    pub const DEVICE_INFO: &str = "device.info";
    pub const DEVICE_ROOT_CHECK: &str = "device.root_check";
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_LIST_LOCALIZED: &str = "package.list_localized";
    pub const ACTIVITY_FOREGROUND: &str = "activity.foreground";
    pub const PROCESS_PORTS: &str = "process.ports";
    pub const PROCESS_BY_PORT: &str = "process.by_port";
    pub const PROCESS_KILL: &str = "process.kill";
    /// 读 `/proc/<pid>/maps|cmdline|status` 的**详情**：点开后才有的一次按需读取。
    pub const PROCESS_PROC_READ: &str = "process.proc_read";
    pub const FILESYSTEM_LIST: &str = "filesystem.list";
    pub const FILESYSTEM_STAT: &str = "filesystem.stat";
    pub const FILESYSTEM_PREVIEW: &str = "filesystem.preview";
    pub const FILESYSTEM_MKDIR: &str = "filesystem.mkdir";
    pub const FILESYSTEM_RENAME: &str = "filesystem.rename";
    pub const FILESYSTEM_REMOVE: &str = "filesystem.remove";
    pub const FILESYSTEM_CHMOD: &str = "filesystem.chmod";
    pub const HOSTED_LIST: &str = "hosted.list";
    pub const HOSTED_CHMOD: &str = "hosted.chmod";
    pub const HOSTED_START: &str = "hosted.start";
    pub const HOSTED_STATUS: &str = "hosted.status";
    pub const HOSTED_STOP: &str = "hosted.stop";
    pub const PACKAGE_EXPORT_APK: &str = "package.export_apk";
    pub const PACKAGE_EXPORT_CLEAN: &str = "package.export_clean";
    pub const PACKAGE_DESCRIBE: &str = "package.describe";
    pub const ZYGISK_STATUS: &str = "zygisk.status";
    pub const PACKAGE_NATIVE_LIB_DIR: &str = "package.native_lib_dir";
    pub const ACTIVITY_LAUNCH: &str = "activity.launch";
    pub const ACTIVITY_FORCE_STOP: &str = "activity.force_stop";
    pub const PACKAGE_UNINSTALL: &str = "package.uninstall";
    pub const PACKAGE_REPLACE_NATIVE_LIBRARY: &str = "package.replace_native_library";
    pub const FRIDA_SERVER_STATUS: &str = "frida.server.status";
    pub const FRIDA_SERVER_START: &str = "frida.server.start";
    pub const FRIDA_SERVER_STOP: &str = "frida.server.stop";
}

/// AR8.4：Desktop 用 `adb push` 暂存「主机侧修补好的 so」的**唯一**允许目录。
///
/// 两侧共用这个常量：Desktop 只往这里推，Agent 只认这里的文件，
/// 避免「推到 A、校验 B」这种靠文档维持的约定。每次操作再用唯一子目录隔开。
pub const SO_STAGED_ROOT: &str = "/data/local/tmp/app-reverse-tools-so";

/// frida-server 在设备上的实际状态（AR9.1）。
///
/// `running_as_shell` 是**必须单独存在**的一档：shell 身份起的 frida-server 能连上，
/// 但 attach 不了别的进程，用户会看到「服务在跑却注入不进去」。把它并进 `running`
/// 就是骗人，所以状态里带 uid 事实而不是一个布尔。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FridaServerState {
    /// 没找到进程
    NotRunning,
    /// 以 uid=0 运行（可用）
    RunningAsRoot,
    /// 以非 root 运行（能连不能注入，需要重启）
    RunningAsShell,
    /// 进程在但读不到身份（权限或竞态），不得猜成前三种
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FridaServerStatusParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FridaServerStatusResult {
    pub state: FridaServerState,
    pub running: bool,
    /// 只有真读到 uid=0 才是 true；读不到一律 false + `Indeterminate`
    pub as_root: bool,
    pub binary_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    /// 从 cmdline 里解析出的 `-l <addr>:<port>`，没有就是 Framework 默认监听
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `/proc/net/tcp*` 里该端口是否真的处于 LISTEN（进程在 ≠ 监听上）
    pub listening: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 启动参数：**只收名字、端口与绑定地址**，路径由设备侧自己在托管目录里解析（D038）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FridaServerStartParams {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `127.0.0.1`（默认）或 `0.0.0.0`。默认回环是有意的：远程模式靠 `adb forward`，
    /// 绑 0.0.0.0 等于把 frida 控制面开放给同网段任何设备。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FridaServerStartResult {
    pub operation_id: String,
    pub outcome: WriteOutcome,
    /// 启动后**复核**过：进程在、uid=0、端口在 LISTEN
    pub verified: bool,
    pub binary_name: String,
    pub bind: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub steps: Vec<OperationStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FridaServerStopParams {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FridaServerStopResult {
    pub operation_id: String,
    pub outcome: WriteOutcome,
    /// 复核过进程真的没了（不是「信号发出去了」）
    pub verified: bool,
    pub binary_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub steps: Vec<OperationStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Zygisk 模块生命周期。`installed_reboot_required` / `loaded` / `bridge_ready` 必须区分，
/// 「文件已装」不等于「接口可用」（AR5.3 契约）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZygiskLifecycle {
    NotInstalled,
    ZygiskDisabled,
    InstalledRebootRequired,
    Loaded,
    BridgeReady,
    Incompatible,
    Faulted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ZygiskStatusParams {}

/// 模块自描述的 handler（AR10.2）。桌面端不拼私有协议，只看这份声明：
/// 每条方法的预算与「当前是否被熔断」是可核对的事实，而不是模块里的黑话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleHandlerInfo {
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub target: String,
    pub permission: String,
    pub timeout_ms: u64,
    pub max_response_bytes: u64,
    pub cancellable: bool,
    pub fused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZygiskStatusResult {
    pub lifecycle: ZygiskLifecycle,
    pub bridge_ready: bool,
    /// 只表示「Agent 能读到模块目录」；root 不可用时为 false，不推断安装状态。
    pub root_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_version_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zygisk_impl: Option<String>,
    /// 设备默认 locale；模块按它解析 label，Agent 不伪造按请求 locale 的结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_locale: Option<String>,
    /// Agent <-> 模块私有子协议版本（当前冻结为 Q/E/D 线协议 = 1）。
    pub sub_protocol_version: u32,
    /// v2 模块在监听，但**子协议版本或应答格式对不上**（模块比本端新、或端口被别的东西占了）。
    ///
    /// 为什么不直接把 lifecycle 改成 `incompatible` 就完事：AR10.5 真机注入发现，
    /// v2 不兼容时 v1 demo 往往还在正常服务，`bridge_ready=true` 是真话（清单确实出得来），
    /// 但界面必须同时知道"你装的那个 v2 模块本端说不了"，否则会一直显示"一切就绪"。
    /// 两个事实都得在场，谁也不能盖掉谁。`default` 让旧 Agent 不带该字段时仍可解析。
    #[serde(default)]
    pub module_incompatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_latency_ms: Option<u64>,
    /// 模块声明的 handler 注册表（AR10.2）。**故意不跳过空数组**：
    /// 空数组=「问过了，模块没有 `handlers` 能力」（旧模块），字段缺失=「这个 Agent
    /// 版本压根没问过」——排障时这两种情况要能分开，所以不能用 skip_serializing_if。
    #[serde(default)]
    pub module_handlers: Vec<ModuleHandlerInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportApkParams {
    pub package_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedApkFile {
    pub name: String,
    pub size: u64,
    /// 设备侧暂存路径，仅供 Desktop 走 ADB pull 传输使用。
    pub remote_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportApkResult {
    pub package_name: String,
    /// 一次性暂存会话标识，Desktop 取回后必须调用 `package.export_clean` 回收。
    pub session: String,
    pub files: Vec<StagedApkFile>,
    pub bytes: u64,
    /// 模块明确跳过的分片（例如 `too_large`）。非空表示这次导出**不完整**，
    /// Desktop 合并出来的 `.apks` 也就缺件，必须让界面说出来而不能装作成功。
    /// `default` 是为了兼容还没带这个字段的旧 Agent。
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// 按包问一次 Framework：本地化显示名、版本号、以及**这个包一共有哪几个 APK**。
///
/// 存在的理由只有一个：这几件事 ADB 答不上来（`pm` 给不出按设备语言解析的名字），
/// 而导出与命名又必须知道它们。批量清单 `package.list_localized` 是 O(全机包数)，
/// 为一个应用跑全机清单不划算，所以补这条单点查询（AR10.3 的第一个真实新接口）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDescribeParams {
    pub package_name: String,
}

/// 设备侧声明的一个 APK 分片（名字保持 `base.apk` / `split_*.apk` 原样）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribedApkFile {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDescribeResult {
    pub package_name: String,
    pub label: String,
    pub label_source: LabelSource,
    pub requested_locale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    /// 设备上真正生效的 locale（模块自报），命名口径的证据字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_locale: Option<String>,
    pub is_system: bool,
    pub enabled: bool,
    /// 这个包声明的全部分片；导出结果比这里少就是缺件
    pub apk_files: Vec<DescribedApkFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportCleanParams {
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExportCleanResult {
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EmptyParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityListResult {
    pub capabilities: Vec<CapabilityInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceInfoParams {}

/// 前台应用（AR6.1）：`dumpsys window`/`pidof`/`/proc` 的解析全部在设备端完成，
/// Desktop 不再拼 shell 字符串。读不到的字段保持 `None`，不用空串或 0 伪装成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    ThirdParty,
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActivityForegroundParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcEntrySummary {
    /// maps | cmdline | status
    pub name: String,
    pub path: String,
    /// maps=行数、cmdline=命令行（截断）、status=头几行；不自动读或读不到为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub readable: bool,
    /// **这一项不自动读，界面上给箭头，点了才走 `process.proc_read`。**
    ///
    /// 为什么需要这个标记而不是直接用 `readable=false`：`/proc/<pid>/maps` 属于另一个
    /// 用户的进程，Agent 以 shell 身份读必然 `EACCES`（Android 14 实测；root 才读得到）。
    /// 把它当"一次普通读取"放进摘要里，结果就是每次刷新设备信息都白跑一次注定失败的
    /// 读取，并在界面上留下一句红色的"不可读"——看起来像工具坏了，其实是权限边界，
    /// 而且用户要的根本不是那一个数字，是内容。所以这里显式区分三态：
    /// 读到了 / 读不到 / **没读（等你点）**。
    #[serde(default)]
    pub read_on_demand: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityForegroundResult {
    /// false = 未解析到前台窗口（锁屏、弹窗或 ROM 输出差异），不算错误
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub package_kind: PackageKind,
    /// `legacyNativeLibraryDir`：部分 ROM 不再输出该字段，缺失即 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lib_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proc: Vec<ProcEntrySummary>,
    /// 无前台时的原因提示，便于 UI 直接展示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// AR10.5 之后的按需读取：`/proc/<pid>/<file>` 的详情。
///
/// `file` 是**枚举**而不是路径字符串：这一条方法在需要时会经 `su` 提权执行（maps 以
/// shell 身份读不到），所以绝不允许调用方传任意路径进来——白名单在 Agent 侧逐值拼接，
/// 参数非法直接 `invalid_request`（与 D038"特权只走写死的固定脚本 + 校验过的参数"同一条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcFile {
    Maps,
    Cmdline,
    Status,
}

impl ProcFile {
    /// 拼进脚本的那一段文件名：只有这三个值，别的都进不来。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Maps => "maps",
            Self::Cmdline => "cmdline",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessProcReadParams {
    pub pid: u32,
    pub file: ProcFile,
    /// 返回行数上限；省略=Agent 默认值，超出会被夹紧（不是报错）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessProcReadResult {
    pub path: String,
    pub file: ProcFile,
    /// 文件总行数（截断与否都以它为准）
    pub total_lines: u64,
    /// 本次真的带回来多少行
    pub returned_lines: u32,
    pub truncated: bool,
    /// 正文。`cmdline` 的 NUL 已经换成空格并截尾
    pub text: String,
    /// `shell` = 普通身份就读到了；`root` = 走了提权。界面要如实标出来，
    /// 因为它同时告诉用户"这台机没授权时会看到什么"
    pub read_via: String,
}

/// AR6.2：端口/进程互查。`/proc/net/*` 解析与 fd→inode 匹配全部在设备端完成，
/// Desktop 不再 cat 全文 + 宿主解析，也不再分批 `ls /proc/*/fd`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessPortsParams {
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningPort {
    pub port: u16,
    /// `/proc/net` 里十六进制地址还原后的可读形式（IPv4 点分 / IPv6 冒分）
    pub address: String,
    pub family: SocketFamily,
    /// `listen`/`time_wait`/... 已按内核 st 值翻译；未收录值保留 `st=<hex>`
    pub state: String,
    pub inode: u64,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPortsResult {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmdline: Option<String>,
    pub ports: Vec<ListeningPort>,
    /// 读不到就列出来（权限不足或进程已退出），不静默当成"没有监听端口"
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketFamily {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessByPortParams {
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortHoldingProcess {
    pub pid: u32,
    pub uid: u32,
    pub family: SocketFamily,
    pub address: String,
    pub state: String,
    pub inode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessByPortResult {
    pub port: u16,
    pub sockets: Vec<PortHoldingProcess>,
    /// 无法确定属主的 socket（未找到持有该 inode 的进程）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unowned: Vec<PortHoldingProcess>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// 候选进程数超过扫描上限时列出被跳过的原因，方便判断"查不到"是权限还是上限
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// AR6.3：写操作 `process.kill`。Desktop 传进来的 PID 只是「意图」，Agent 执行前
/// 必须重读 `/proc/<pid>` 身份，对不上就拒止（PID 复用可能杀掉刚起来的无关进程）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillSignal {
    #[default]
    Term,
    /// 对应 `kill -9`：Legacy 托管进程停止用的就是它，迁移期保持等价
    Kill,
}

impl KillSignal {
    pub fn number(self) -> i32 {
        match self {
            Self::Term => 15,
            Self::Kill => 9,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessKillParams {
    pub pid: u32,
    /// 期望进程名（`comm` 或 cmdline 可执行文件名）；缺省表示不做身份校验
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_comm: Option<String>,
    #[serde(default)]
    pub signal: KillSignal,
    /// 调用方声明这次终止需要 root。Agent 自己仍以 shell 身份运行，**不是**"试一下失败再说"
    /// （那会把权限问题伪装成进程问题），而是改跑一条带身份核验的固定提权脚本：
    /// 脚本先比 `/proc/<pid>/comm` 再发信号，名字对不上就一个信号都不发。
    /// su 不可用时仍然如实报 `permission_denied`。
    #[serde(default)]
    pub require_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillOutcome {
    /// 信号已送达
    Signaled,
    /// 目标本来就不存在（幂等成功：重复点击不该报错）
    AlreadyGone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessKillResult {
    pub pid: u32,
    pub signal: KillSignal,
    pub outcome: KillOutcome,
    /// 执行前重读到的身份证据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmdline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub ran_as_root: bool,
    /// 发信号后是否确认进程消失；`false` 只代表「没确认到」，不代表「一定还活着」
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified_dead: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// AR7.1：设备端文件 API。Desktop 不再解析 `ls -l` 文本，也不再 `head -c` 拉正文；
/// 类型/权限/属主/时间戳全部由 Agent 用 `lstat`+`readlink` 直接取，时间固定 Unix epoch 秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Dir,
    File,
    Symlink,
    Socket,
    Fifo,
    Block,
    Char,
    Other,
}

impl FileKind {
    /// `ls -l` 权限串首字符，便于与 Legacy 输出逐字对照。
    pub fn type_char(self) -> char {
        match self {
            Self::Dir => 'd',
            Self::File => '-',
            Self::Symlink => 'l',
            Self::Socket => 's',
            Self::Fifo => 'p',
            Self::Block => 'b',
            Self::Char => 'c',
            Self::Other => '?',
        }
    }

    /// 由 `st_mode` 的文件类型位（S_IFMT）判定；未知类型归 `other`，绝不猜成普通文件。
    /// 用裸掩码而不是 `std::os::unix`，协议 crate 在三平台（含 Windows 宿主编译）都可用。
    pub fn from_mode(mode: u32) -> Self {
        match mode & S_IFMT {
            S_IFDIR => Self::Dir,
            S_IFREG => Self::File,
            S_IFLNK => Self::Symlink,
            S_IFSOCK => Self::Socket,
            S_IFIFO => Self::Fifo,
            S_IFBLK => Self::Block,
            S_IFCHR => Self::Char,
            _ => Self::Other,
        }
    }
}

/// POSIX `st_mode` 文件类型位与特殊权限位（与 libc 常量同值，避免协议 crate 依赖平台扩展）。
pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFIFO: u32 = 0o010000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_ISUID: u32 = 0o4000;
pub const S_ISGID: u32 = 0o2000;
pub const S_ISVTX: u32 = 0o1000;
/// `FileStat.mode` 只保留权限位（含 setuid/setgid/sticky），不含文件类型位。
pub const PERMISSION_BITS: u32 = 0o7777;

/// 渲染 `ls -l` 风格的权限串（`-rwxr-xr-x`、`drwxrwx--x`、`lrwxrwxrwx`、`-rwsr-xr-x`、`drwxrwxrwt`）。
/// Agent 与 Desktop 共用同一实现，shadow 对照才不会把渲染差异当成结果差异。
///
/// setuid/setgid/sticky 按 `ls` 的规则覆盖对应三元的执行位：有执行位用小写 `s`/`t`，
/// 没有执行位用大写 `S`/`T`。少了这一步，`/system/bin` 下的 setuid 文件会与
/// Legacy `ls -lA` 输出对不上，shadow 会把渲染差异误报成结果差异（同 AR6.2 的 IPv6 记法坑）。
pub fn render_mode_text(kind: FileKind, mode: u32) -> String {
    let mut text = String::with_capacity(10);
    text.push(kind.type_char());
    for (triplet, special, marker) in [
        ((mode >> 6) & 0o7, mode & S_ISUID, 's'),
        ((mode >> 3) & 0o7, mode & S_ISGID, 's'),
        (mode & 0o7, mode & S_ISVTX, 't'),
    ] {
        text.push(if triplet & 0o4 != 0 { 'r' } else { '-' });
        text.push(if triplet & 0o2 != 0 { 'w' } else { '-' });
        text.push(match (triplet & 0o1 != 0, special != 0) {
            (true, true) => marker,
            (true, false) => 'x',
            (false, true) => marker.to_ascii_uppercase(),
            (false, false) => '-',
        });
    }
    text
}

/// 单条目录项 / 单文件元数据。`symlink_target` 是 `readlink` 原值（未解析），
/// 与 `ls -l` 显示一致；`kind` 也按 lstat 判定，指向目录的符号链接仍是 `symlink`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    pub name: String,
    pub kind: FileKind,
    /// 权限位（不含文件类型位，含 setuid/setgid/sticky），如 0o755、0o4755
    pub mode: u32,
    /// `-rwxr-xr-x` 形式，首字符为类型字符
    pub mode_text: String,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    /// Unix epoch 秒；固定单位与时区，不返回本地格式字符串
    pub mtime_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    /// 当前 Agent 身份能否读内容（目录=能否列举）。读不到时 size/mtime 仍可能有效，
    /// 但不能把「读不到」当成「空文件/空目录」
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FilesystemListParams {
    pub path: String,
    /// `true` = 含隐藏项（`ls -lA` 语义）；`false`（缺省）= 只返回非隐藏项。
    /// 两种取值都不含 `.` 与 `..`。要与 Legacy `ls -lA` 对齐的调用方必须显式传 `true`。
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemListResult {
    /// 规范化（符号链接已解析）后的目录路径
    pub path: String,
    pub entries: Vec<FileStat>,
    /// 超过上限被截断时为 true，调用方必须知道列表不完整
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// 逐项失败证据（单项 lstat 失败不影响整目录）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FilesystemStatParams {
    pub path: String,
    /// true = 解析符号链接后取目标元数据（stat），false = lstat 语义
    #[serde(default)]
    pub follow_symlink: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemStatResult {
    /// 调用方原始输入，审计与排障用
    pub requested_path: String,
    /// 规范化后的真实路径（符号链接已解析）
    pub path: String,
    pub stat: FileStat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewEncoding {
    /// 合法 UTF-8（或 ASCII）文本
    Utf8,
    /// 含 NUL 或非法 UTF-8：小写十六进制，不用 base64（可直接肉眼比对且不引入额外字母表）
    Hex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FilesystemPreviewParams {
    pub path: String,
    /// 缺省 64 KiB，硬上限 256 KiB：大文件绝不整块塞进 JSON
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u32>,
    /// true = 从文件尾读取（日志尾读语义，替代 `tail -c`）
    #[serde(default)]
    pub from_end: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemPreviewResult {
    pub path: String,
    /// 文件总大小
    pub size: u64,
    /// 本次返回内容在文件中的起始偏移
    pub offset: u64,
    pub returned_bytes: u32,
    pub encoding: PreviewEncoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// 文件比返回内容大（受 max_bytes 限制）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// AR7.2：托管二进制。Desktop 不再 `ls -l` + `file` + `nohup … & echo $!`，
/// 生命周期与身份判定全在设备端完成。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedBinaryInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    /// 权限位（含 setuid/sticky），不含文件类型位
    pub mode: u32,
    pub mode_text: String,
    /// owner 有执行位（UI 的绿色/红色标记）
    pub has_exec: bool,
    pub uid: u32,
    pub mtime_unix: u64,
    /// 设备上**正在跑、但不在本工具托管表里**的同名进程。
    ///
    /// 为什么不止带 pid、还要带 ppid 与 uid：光一个 pid 分不出两种完全不同的情况 —
    /// ① 别人（或用户自己 `su -c`）起的；② 本工具起的那个进程**自己 fork 成了守护进程**，
    /// 父进程一退，运行表按"我起的那个已退出"清掉记录，活着的子进程就成了"表外进程"
    /// （用户现场的 `auth-server` 正是这种：`ppid=1`、`uid=0`）。`ppid=1` 是②的形状，
    /// 带出来界面才能说实话，而不是替用户断言"这不是你起的"。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_procs: Vec<ExternalProc>,
}

/// 一个"在跑但不在托管表里"的同名进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalProc {
    pub pid: u32,
    /// 父进程 pid。`1` = 父进程已退出、被 init 收养（守护进程与 fork 子进程的共同形状）
    pub ppid: u32,
    /// 真实 uid（`0` = root 起的；Agent 以 shell 身份发不动信号，只能走提权通道）
    pub uid: u32,
}

/// 一次托管运行的状态。`unknown` 只用于「Agent 重启后连 `/proc` 都读不到」，
/// 绝不拿它冒充 `running`，也不冒充 `exited`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedRunState {
    Running,
    Exited,
    Unknown,
}

/// 托管运行记录。身份是 `pid + start_time_ticks`（`/proc/<pid>/stat` 第 22 字段），
/// 单靠 PID 在 PID 复用后会认错进程；`handle` 是给上层用的稳定句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedRunRecord {
    pub handle: String,
    pub name: String,
    pub pid: u32,
    pub start_time_ticks: u64,
    pub started_at_unix: u64,
    pub log_path: String,
    /// 由谁启动：Agent 只能以 shell 身份启动（root=true 的记录来自 Legacy 支路对账）
    pub root: bool,
    pub state: HostedRunState,
    /// 仅 Agent 亲自启动、且已回收僵尸时才有的退出码
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostedListParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedListResult {
    pub dir: String,
    /// 只含 ELF 头的普通文件（按文件头 magic 判定，不再依赖设备端 `file` 命令）
    pub binaries: Vec<HostedBinaryInfo>,
    pub runs: Vec<HostedRunRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostedChmodParams {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedChmodResult {
    pub name: String,
    pub path: String,
    pub mode: u32,
    pub mode_text: String,
    pub has_exec: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostedStartParams {
    pub name: String,
    /// 参数数组直接交给 `exec`，不经过 shell；文件名与参数都不进任何解析上下文
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// 调用方要求以 root 启动：Agent 以 shell 身份运行时应显式拒绝（同 D026）
    #[serde(default)]
    pub root: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedStartResult {
    pub record: HostedRunRecord,
}

/// AR7.3：停止托管进程。用 `handle` 而不是裸 PID 寻址，并在发信号前用落盘的
/// start time 再核一次身份——PID 复用窗口里，按数字杀进程可能杀到别人。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostedStopParams {
    pub handle: String,
    /// 调用方看到的 PID：与记录不一致说明中间发生过复用，直接拒止
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_pid: Option<u32>,
    #[serde(default)]
    pub signal: KillSignal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedStopResult {
    pub record: HostedRunRecord,
    pub outcome: KillOutcome,
    /// 已核对过 `/proc` 的 start time（true 表示「确认杀的就是当初启动的那个进程」）
    pub identity_verified: bool,
    /// 记录文件是否已从磁盘删除（停止成功后不再占用重启对账表）
    pub record_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostedStatusParams {
    pub handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedStatusResult {
    pub record: HostedRunRecord,
    /// 记录是否来自磁盘对账（Agent 重启后不再是自己的子进程，无法回收退出码）
    pub reconciled: bool,
}

/// AR8.3：包安装目录里的 native lib 路径解析（只读）。Desktop 之前是
/// `dumpsys package <pkg>` 全文回宿主再按字符串找 `legacyNativeLibraryDir=`，
/// 这里改为 Agent 侧解析并给出可核对的来源证据。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageNativeLibDirParams {
    pub package: String,
    /// `arm64` | `arm`；缺省时按包声明的 `primaryCpuAbi` 判定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<String>,
    /// 多用户安装时指定 user id；缺省取第一个可见实例
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<u32>,
}

/// 路径是怎么来的必须能区分：Framework 明确给了 `legacyNativeLibraryDir`，
/// 和我们从 `codePath` 推出来，可信度不是一回事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeLibDirSource {
    FrameworkField,
    DerivedFromCodePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageNativeLibDirResult {
    pub package: String,
    pub native_lib_dir: String,
    /// 最终采用的 ABI 口径（`arm64` | `arm`）
    pub abi: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_cpu_abi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_path: Option<String>,
    /// 命中的 split APK 路径（无 split 时为空数组，不省略字段：区分「没有」与「没看」）
    pub splits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<u32>,
    pub source: NativeLibDirSource,
    /// 多个用户实例给出不同目录等情况下，说明我们是怎么选的
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// AR8.1：包写操作。三个方法共用一套语义——写操作不自动回退、必须带 `operation_id`，
/// 同一个 id 重复提交只返回已知结果，绝不二次执行（网络重试、用户连点都不该再动一次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageWriteAction {
    Launch,
    ForceStop,
    Uninstall,
}

/// `replayed` 与 `executed` 必须能区分：前者是幂等命中，后者是真的动过设备状态。
/// `no_op` 表示目标已经在期望状态（例如强停一个本来没在跑的包）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    Executed,
    Replayed,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActivityLaunchParams {
    pub package: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActivityForceStopParams {
    pub package: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PackageUninstallParams {
    pub package: String,
    pub operation_id: String,
    /// `pm uninstall -k`：保留数据与缓存
    #[serde(default)]
    pub keep_data: bool,
    /// 指定用户卸载；缺省为全部用户
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<u32>,
}

/// 破坏性操作的单步结果（AR8.1/AR8.4）：失败时用户要能看出**卡在哪一步**，
/// 而不是只拿到一句「卸载失败」。`ok=false` 的步骤必须带 `detail`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationStep {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 卸载结果。比通用 `PackageWriteResult` 多带步骤链，因为卸载是**不可逆**动作，
/// 「pm 说 Success」和「路径真的没了」「数据是否按 keep_data 保留」是三件不同的事。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageUninstallResult {
    pub package: String,
    pub operation_id: String,
    pub outcome: WriteOutcome,
    /// 复核结论：`pm path` 已为空
    pub verified: bool,
    pub keep_data: bool,
    pub steps: Vec<OperationStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// AR8.4：把主机侧已修补的 `.so` 原子装到包的 native lib 目录。
/// `staged_path` 一定是 Desktop 用 ADB push 上去、由本方法校验过的暂存文件；
/// Agent 不接收任意命令，只执行代码里写死的固定脚本（D038）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReplaceNativeLibraryParams {
    pub package: String,
    /// `arm64` | `arm`
    pub abi: String,
    /// 形如 `libfoo.so`：只允许 `[A-Za-z0-9._-]`，必须 `.so` 结尾
    pub so_name: String,
    /// 设备侧暂存文件（/data/local/tmp 下）
    pub staged_path: String,
    pub operation_id: String,
}

/// 单步结果里的证据按需给：`sha256_before`/`sha256_after` 只有真正算出来才有值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceNativeLibraryResult {
    pub package: String,
    pub target_path: String,
    pub staged_path: String,
    pub operation_id: String,
    pub outcome: WriteOutcome,
    /// 复核结论：目标的 sha256 与暂存件一致、大小一致、ELF magic 正确
    pub verified: bool,
    /// 目标是「替换已有」还是「新增文件」——回滚动作完全不同
    pub replaced_existing: bool,
    pub steps: Vec<OperationStep>,
    /// 失败且已自动恢复备份时为 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageWriteResult {
    pub action: PackageWriteAction,
    pub package: String,
    pub operation_id: String,
    pub outcome: WriteOutcome,
    /// 执行后客观复核过（launch 看到 pid / force_stop 看到 pid 消失 / uninstall 看到 pm path 为空）
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// am/pm 都是 shell 身份发起；root 支路（`su -c`）在 AR8 之前不承诺
    pub ran_as_root: bool,
}

/// AR9.1 前置：设备 root 能力探测改由 Agent 执行（Desktop 之前自己跑 `su -c id`）。
/// 这是 §2.1「Desktop 不把在线等同于 root」那条要求落地的地方。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceRootCheckParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRootCheckResult {
    /// 只在**确实拿到 uid=0** 时为 true；超时、su 不存在、被拒都算 false
    pub root: bool,
    /// Agent 自身 uid（未提权时就是它真实身份，便于解释为什么读不到别的进程）
    pub agent_uid: u32,
    /// 探测耗时，便于 UI 区分「问过了」和「没问出来」
    pub probe_ms: u64,
    /// 区分 false 的原因：`su_unavailable`（二进制不可达）、`denied`（有 su 但没给 root）、
    /// `timeout`（授权弹窗未答或卡住）、`granted`。false 不等于「设备没 root」，
    /// 也可能是用户还没点允许——UI 要能说出来
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfoResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_level: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_abi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wlan_ipv4: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageScope {
    All,
    User,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListParams {
    pub scope: PackageScope,
    pub include_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSummary {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub is_system: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListResult {
    pub items: Vec<PackageSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListLocalizedParams {
    /// `None` = 使用设备默认 locale（手机是中文就返回中文清单）；
    /// 指定 locale 且与设备默认不一致时，条目会带 `fallback_reason`，不冒充已按请求 locale 解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub scope: PackageScope,
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    Framework,
    Manifest,
    PackageName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizedPackageItem {
    pub package_name: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    pub requested_locale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_locale: Option<String>,
    pub label_source: LabelSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub is_system: bool,
    pub enabled: bool,
}

// ===== 文件页写操作（增删改）=====
//
// 这四条是"读三件套"（list/stat/preview）之外的写侧能力。口径与 AR7.1 一致：
// 请求里只放**路径与模式**，不放任何 shell 文本；执行由 Agent 用 `std::fs` 做，
// 完成后**必须读回复核**再返回，返回码不是结论。写操作的允许根与读侧**不同**，
// 默认收窄到 `/sdcard` 与 `/data/local/tmp`（见 filesystem provider 的注释）。

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemMkdirParams {
    /// 要新建的目录完整路径（父目录必须已存在：不做 mkdir -p，那会把打错的路径变成"成功"）
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemMkdirResult {
    pub path: String,
    /// false 表示这个目录本来就在（幂等命中，没有改动设备）
    pub created: bool,
    pub mode: u32,
    pub mode_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemRenameParams {
    pub from: String,
    /// 完整目标路径（同一目录内改名时也只有 `to` 变最后一段；跨目录即"移动"）
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemRenameResult {
    pub from: String,
    pub to: String,
    pub kind: FileKind,
    pub mode: u32,
    pub mode_text: String,
    /// true 表示 from 与 to 是同一个位置（无需改动，幂等命中）
    pub no_op: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemRemoveParams {
    pub path: String,
    /// 删目录必须显式声明递归：默认 false 时非空目录直接失败，
    /// 不猜"用户大概想连里面的一起删"。
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemRemoveResult {
    pub path: String,
    /// 复核结论：再 stat 一次，确实不存在了才 true
    pub removed: bool,
    pub was_dir: bool,
    pub was_recursive: bool,
    /// 删掉的字节数（目录取自身大小，递归时累计），给界面一句"释放了多少"
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemChmodParams {
    pub path: String,
    /// 只接受低 12 位（含 setuid/setgid/sticky）；其余位为非法请求
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemChmodResult {
    pub path: String,
    pub mode: u32,
    pub mode_text: String,
    pub previous_mode: u32,
    pub previous_mode_text: String,
    /// 读回来的 mode 与请求一致
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageWarning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageListLocalizedResult {
    pub items: Vec<LocalizedPackageItem>,
    pub success_count: u32,
    pub fallback_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<PackageWarning>,
    /// 实际服务本次清单的通道，UI 必须可见：`zygisk_v2`（可指定 locale）、
    /// `zygisk_v1`（demo 模块，只有设备默认 locale）或 `zygisk_none`。
    /// 缺字段表示旧版 Agent，前端按 `zygisk_none` 处理，不猜成 v2。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

#[cfg(test)]
mod tests {

    #[test]
    fn proc_read_dto_is_snake_case_and_the_file_enum_is_closed() {
        let params = ProcessProcReadParams {
            pid: 4321,
            file: ProcFile::Maps,
            max_lines: Some(400),
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["file"], "maps");
        assert_eq!(value["max_lines"], 400);
        // 省略 max_lines 时不出现该字段（旧 Desktop 发的请求体形状不变）
        let omitted = serde_json::to_value(ProcessProcReadParams {
            pid: 1,
            file: ProcFile::Cmdline,
            max_lines: None,
        })
        .unwrap();
        assert_eq!(omitted.get("max_lines"), None);
        // 只认这三个值：想借这条方法读别的路径，反序列化阶段就该失败
        assert!(serde_json::from_value::<ProcFile>(serde_json::json!("environ")).is_err());
        assert!(serde_json::from_value::<ProcFile>(serde_json::json!("../cmdline")).is_err());

        let result = ProcessProcReadResult {
            path: "/proc/4321/maps".into(),
            file: ProcFile::Maps,
            total_lines: 2557,
            returned_lines: 2,
            truncated: true,
            text: "a\nb".into(),
            read_via: "root".into(),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["total_lines"], 2557);
        assert_eq!(value["returned_lines"], 2);
        assert_eq!(value["read_via"], "root");
    }

    #[test]
    fn proc_summary_treats_missing_read_on_demand_as_eager_read() {
        // 旧 Agent 不带这个字段：必须照旧当"已经读过的摘要"，不能凭空变成"等你点"
        let legacy = serde_json::json!({
            "name": "cmdline",
            "path": "/proc/1/cmdline",
            "summary": "init",
            "readable": true
        });
        let parsed: ProcEntrySummary = serde_json::from_value(legacy).unwrap();
        assert!(!parsed.read_on_demand);
        assert_eq!(parsed.summary.as_deref(), Some("init"));
    }

    use crate::{AgentError, ErrorCode};
    use serde_json::json;

    use super::*;

    #[test]
    fn package_params_and_label_source_use_snake_case_values() {
        let params = PackageListLocalizedParams {
            locale: Some("zh-CN".into()),
            scope: PackageScope::User,
            include_disabled: true,
        };
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["scope"], "user");
        assert_eq!(
            serde_json::to_value(LabelSource::PackageName).unwrap(),
            "package_name"
        );
    }

    #[test]
    fn localized_result_preserves_fallback_and_omits_absent_optional_fields() {
        let result = PackageListLocalizedResult {
            items: vec![LocalizedPackageItem {
                package_name: "com.example.app".into(),
                label: "Example".into(),
                version_name: Some("1.2.3".into()),
                version_code: Some(45),
                requested_locale: "zh-CN".into(),
                resolved_locale: None,
                label_source: LabelSource::Manifest,
                fallback_reason: Some("no_zh_resource".into()),
                uid: None,
                is_system: false,
                enabled: true,
            }],
            success_count: 1,
            fallback_count: 1,
            warnings: vec![],
            channel: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["items"][0]["label_source"], "manifest");
        assert_eq!(value["items"][0]["fallback_reason"], "no_zh_resource");
        assert_eq!(value["items"][0].get("resolved_locale"), None);
        assert_eq!(value.get("warnings"), None);
        assert_eq!(value["success_count"], json!(1));
    }

    #[test]
    fn foreground_result_is_snake_case_and_omits_absent_fields() {
        let result = ActivityForegroundResult {
            found: true,
            package_name: Some("com.target.app".into()),
            activity: Some("com.target.app.ui.HomeActivity".into()),
            pid: Some(4321),
            package_kind: PackageKind::ThirdParty,
            native_lib_dir: None,
            proc: vec![ProcEntrySummary {
                name: "maps".into(),
                path: "/proc/4321/maps".into(),
                summary: Some("118".into()),
                readable: true,
                read_on_demand: false,
            }],
            hint: None,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["package_name"], "com.target.app");
        assert_eq!(value["package_kind"], "third_party");
        assert_eq!(value["proc"][0]["readable"], true);
        assert_eq!(value.get("native_lib_dir"), None);
        assert_eq!(value.get("hint"), None);

        let empty = ActivityForegroundResult {
            found: false,
            package_name: None,
            activity: None,
            pid: None,
            package_kind: PackageKind::Unknown,
            native_lib_dir: None,
            proc: Vec::new(),
            hint: Some("未解析到前台窗口".into()),
        };
        let value = serde_json::to_value(empty).unwrap();
        assert_eq!(value["package_kind"], "unknown");
        assert_eq!(value.get("proc"), None);
        let parsed: ActivityForegroundResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.package_kind, PackageKind::Unknown);
    }

    #[test]
    fn zygisk_status_lifecycle_is_snake_case_and_omits_absent_fields() {
        let result = ZygiskStatusResult {
            lifecycle: ZygiskLifecycle::InstalledRebootRequired,
            bridge_ready: false,
            root_available: true,
            module_id: Some("applist".into()),
            module_version: Some("v1.0".into()),
            module_version_code: Some(1),
            zygisk_impl: Some("zygisksu".into()),
            device_locale: Some("zh-Hans-CN".into()),
            sub_protocol_version: 1,
            probe_latency_ms: Some(4),
            module_handlers: Vec::new(),
            detail: None,
            module_incompatible: false,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["lifecycle"], "installed_reboot_required");
        assert_eq!(value["module_id"], "applist");
        assert_eq!(value["sub_protocol_version"], 1);
        assert_eq!(value.get("detail"), None);
        // handler 注册表即使为空也必须在线上传过去：空数组=「问过了，模块没有」，
        // 字段缺失=「这个 Agent 版本没问」。少了这条断言，以后有人加
        // skip_serializing_if 就会把两种情况糊成一个。
        assert_eq!(
            value["module_handlers"]
                .as_array()
                .expect("必须有线上字段")
                .len(),
            0,
            "空注册表也要序列化出来"
        );
    }

    #[test]
    fn hosted_run_record_keeps_state_and_exit_code_honest() {
        let running = HostedRunRecord {
            handle: "aabbccdd00112233".into(),
            name: "toybox".into(),
            pid: 4321,
            start_time_ticks: 987654,
            started_at_unix: 1_760_000_000,
            log_path: "/data/local/tmp/.toybox.run.log".into(),
            root: false,
            state: HostedRunState::Running,
            exit_code: None,
            detail: None,
        };
        let value = serde_json::to_value(&running).unwrap();
        assert_eq!(value["state"], "running");
        assert_eq!(value["start_time_ticks"], json!(987654));
        // 没有回收到的退出码不得出现字段，前端不能拿 null 当 0
        assert_eq!(value.get("exit_code"), None);
        assert_eq!(value.get("detail"), None);
        let parsed: HostedRunRecord = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.state, HostedRunState::Running);
        assert_eq!(parsed.exit_code, None);
        assert_eq!(
            serde_json::to_value(HostedRunState::Unknown).unwrap(),
            json!("unknown")
        );
    }

    #[test]
    fn hosted_params_default_to_shell_start_with_no_args() {
        let params: HostedStartParams =
            serde_json::from_value(json!({ "name": "toybox" })).unwrap();
        assert!(params.args.is_empty());
        assert!(!params.root);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("args"), None, "空参数数组不占报文");
        assert_eq!(value["root"], json!(false));

        let listed = HostedListResult {
            dir: "/data/local/tmp".into(),
            binaries: vec![HostedBinaryInfo {
                name: "toybox".into(),
                path: "/data/local/tmp/toybox".into(),
                size: 4096,
                mode: 0o755,
                mode_text: render_mode_text(FileKind::File, 0o755),
                has_exec: true,
                uid: 2000,
                mtime_unix: 1_760_000_000,
                // 表外同名进程：真机形状（auth-server 活着的那个 ppid=1、uid=0）
                external_procs: vec![ExternalProc {
                    pid: 9727,
                    ppid: 1,
                    uid: 0,
                }],
            }],
            runs: vec![HostedRunRecord {
                state: HostedRunState::Exited,
                exit_code: Some(137),
                ..running_record()
            }],
            truncated: false,
            unreadable: vec![],
        };
        let value = serde_json::to_value(&listed).unwrap();
        assert_eq!(value["binaries"][0]["mode"], json!(493));
        assert_eq!(value["binaries"][0]["mode_text"], "-rwxr-xr-x");
        assert_eq!(value["runs"][0]["exit_code"], json!(137));
        // 外部同名进程：有值才上 wire（snake_case），空数组整个键省略
        let ext = &value["binaries"][0]["external_procs"][0];
        assert_eq!(ext["pid"], json!(9727));
        assert_eq!(
            ext["ppid"],
            json!(1),
            "ppid 要上 wire，界面靠它区分\"被 init 收养\""
        );
        assert_eq!(
            ext["uid"],
            json!(0),
            "uid 要上 wire：root 起的进程 shell 发不动信号"
        );
        let empty = HostedListResult {
            binaries: vec![HostedBinaryInfo {
                external_procs: Vec::new(),
                ..listed.binaries[0].clone()
            }],
            ..listed.clone()
        };
        assert_eq!(
            serde_json::to_value(empty).unwrap()["binaries"][0].get("external_procs"),
            None,
            "没有外部进程时不该在报文里占一个空数组"
        );
        assert_eq!(value.get("truncated"), None);
        assert_eq!(value.get("unreadable"), None);
    }

    /// 写操作守卫字段名必须钉死：`expected_pid` 拼错不会报错，而是守卫静默失效，
    /// 所以协议测试直接把 wire 形状锁住（Agent 侧也有一条对称断言）。
    /// AR8.4 契约：新增与替换必须分得开（回滚动作不同），且 `rolled_back`
    /// 只在真的发生恢复时才有值 —— 不能拿 false 冒充「不需要回滚」。
    #[test]
    fn replace_native_library_result_distinguishes_add_from_replace() {
        let result = ReplaceNativeLibraryResult {
            package: "com.x".into(),
            target_path: "/data/app/~~a/com.x-b/lib/arm64/libfoo.so".into(),
            staged_path: "/data/local/tmp/app-reverse-tools-so/com.x-1c/libfoo.so".into(),
            operation_id: "op-11".into(),
            outcome: WriteOutcome::Executed,
            verified: true,
            replaced_existing: true,
            steps: vec![OperationStep {
                name: "verify_staged".into(),
                ok: true,
                detail: Some("size=10096".into()),
            }],
            rolled_back: None,
            backup_path: Some("/data/local/tmp/app-reverse-tools-so/com.x-1c/libfoo.so.bak".into()),
            detail: Some("sha256_matched".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["replaced_existing"], json!(true));
        assert_eq!(value.get("rolled_back"), None);
        assert_eq!(value["steps"][0]["name"], "verify_staged");
        let parsed: ReplaceNativeLibraryResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, result);
        let rolled: ReplaceNativeLibraryResult = ReplaceNativeLibraryResult {
            rolled_back: Some(true),
            ..parsed
        };
        assert_eq!(
            serde_json::to_value(rolled).unwrap()["rolled_back"],
            json!(true)
        );
    }

    #[test]
    fn root_check_result_keeps_the_reason_for_absence() {
        let result = DeviceRootCheckResult {
            root: false,
            agent_uid: 2000,
            probe_ms: 12,
            detail: Some("su_unavailable".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["root"], json!(false));
        assert_eq!(value["detail"], "su_unavailable");
        // 旧报文没有 detail 时必须解成 None，不能默认成 granted/denied
        let legacy: DeviceRootCheckResult = serde_json::from_value(json!({
            "root": true, "agent_uid": 0, "probe_ms": 30
        }))
        .unwrap();
        assert!(legacy.root);
        assert_eq!(legacy.detail, None);
    }

    /// 卸载结果必须带步骤链，且「数据是否保留」不能靠猜 —— `keep_data` 要原样回显。
    #[test]
    fn uninstall_result_carries_step_chain_and_keep_data_echo() {
        let result = PackageUninstallResult {
            package: "com.x".into(),
            operation_id: "op-9".into(),
            outcome: WriteOutcome::Executed,
            verified: true,
            keep_data: true,
            steps: vec![
                OperationStep {
                    name: "guard_target".into(),
                    ok: true,
                    detail: None,
                },
                OperationStep {
                    name: "pm_uninstall".into(),
                    ok: true,
                    detail: Some("Success".into()),
                },
                OperationStep {
                    name: "verify_removed".into(),
                    ok: true,
                    detail: None,
                },
            ],
            detail: Some("path_gone".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["steps"].as_array().map(Vec::len), Some(3));
        assert_eq!(value["steps"][1]["ok"], json!(true));
        assert_eq!(value["steps"][0].get("detail"), None);
        assert_eq!(value["keep_data"], json!(true));
        let parsed: PackageUninstallResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn package_write_results_separate_executed_from_replayed_and_noop() {
        let result = PackageWriteResult {
            action: PackageWriteAction::ForceStop,
            package: "com.x".into(),
            operation_id: "op-1".into(),
            outcome: WriteOutcome::Executed,
            verified: true,
            pid: None,
            detail: Some("pid_gone".into()),
            ran_as_root: false,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["action"], "force_stop");
        assert_eq!(value["outcome"], "executed");
        assert_eq!(value.get("pid"), None);
        assert_eq!(value["ran_as_root"], json!(false));
        for (outcome, wire) in [
            (WriteOutcome::Replayed, "replayed"),
            (WriteOutcome::NoOp, "no_op"),
        ] {
            assert_eq!(serde_json::to_value(outcome).unwrap(), json!(wire));
        }
    }

    #[test]
    fn write_params_require_operation_id_and_default_flags() {
        let params: PackageUninstallParams =
            serde_json::from_value(json!({ "package": "com.x", "operation_id": "op-2" })).unwrap();
        assert!(!params.keep_data);
        assert_eq!(params.user, None);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("user"), None);
        // 缺 operation_id 必须解不出来：写操作不允许匿名提交
        assert!(
            serde_json::from_value::<ActivityLaunchParams>(json!({ "package": "com.x" })).is_err()
        );
    }

    #[test]
    fn hosted_stop_params_wire_shape_is_snake_case() {
        let params: HostedStopParams = serde_json::from_value(json!({
            "handle": "aabbccddeeff0011", "expected_pid": 4242, "signal": "kill"
        }))
        .unwrap();
        assert_eq!(params.expected_pid, Some(4242));
        assert_eq!(params.signal, KillSignal::Kill);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["expected_pid"], json!(4242));
        assert_eq!(value["signal"], "kill");
        let minimal: HostedStopParams = serde_json::from_value(json!({ "handle": "aa" })).unwrap();
        assert_eq!(minimal.expected_pid, None);
        assert_eq!(minimal.signal, KillSignal::Term);
        let camel: HostedStopParams =
            serde_json::from_value(json!({ "handle": "aa", "expectedPid": 1 })).unwrap();
        assert_eq!(camel.expected_pid, None, "驼峰写法不被识别：守卫会静默失效");

        let result = HostedStopResult {
            record: running_record(),
            outcome: KillOutcome::Signaled,
            identity_verified: true,
            record_dropped: true,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["outcome"], "signaled");
        assert_eq!(value["identity_verified"], json!(true));
        assert_eq!(value["record_dropped"], json!(true));
    }

    fn running_record() -> HostedRunRecord {
        HostedRunRecord {
            handle: "aabbccdd00112233".into(),
            name: "toybox".into(),
            pid: 4321,
            start_time_ticks: 987654,
            started_at_unix: 1_760_000_000,
            log_path: "/data/local/tmp/.toybox.run.log".into(),
            root: false,
            state: HostedRunState::Running,
            exit_code: None,
            detail: None,
        }
    }

    /// AR8.3：`splits` 必须是数组而不是 Option——「没有 split」与「没看 split」
    /// 在 SO 替换页面上是两个完全不同的提示。
    #[test]
    fn native_lib_dir_result_keeps_source_and_empty_splits_visible() {
        let params: PackageNativeLibDirParams =
            serde_json::from_value(json!({ "package": "com.x" })).unwrap();
        assert_eq!(params.abi, None);
        assert_eq!(params.user, None);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("abi"), None);

        let result = PackageNativeLibDirResult {
            package: "com.x".into(),
            native_lib_dir: "/data/app/~~x/com.y-==/lib/arm64".into(),
            abi: "arm64".into(),
            primary_cpu_abi: Some("arm64-v8a".into()),
            code_path: Some("/data/app/~~x/com.y-==".into()),
            splits: vec![],
            user: Some(0),
            source: NativeLibDirSource::FrameworkField,
            detail: Some("multiple_package_blocks=2".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["source"], "framework_field");
        assert_eq!(value["splits"].as_array().map(Vec::len), Some(0));
        assert_eq!(value["user"], json!(0));
        let parsed: PackageNativeLibDirResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.source, NativeLibDirSource::FrameworkField);
        assert!(parsed.splits.is_empty());
        assert_eq!(
            serde_json::to_value(NativeLibDirSource::DerivedFromCodePath).unwrap(),
            json!("derived_from_code_path")
        );
    }

    #[test]
    fn file_kind_and_mode_text_match_ls_conventions() {
        assert_eq!(FileKind::from_mode(0o040755), FileKind::Dir);
        assert_eq!(FileKind::from_mode(0o100644), FileKind::File);
        assert_eq!(FileKind::from_mode(0o120777), FileKind::Symlink);
        assert_eq!(FileKind::from_mode(0o140777), FileKind::Socket);
        // 未知类型位不得猜成普通文件
        assert_eq!(FileKind::from_mode(0o170000), FileKind::Other);
        assert_eq!(render_mode_text(FileKind::Dir, 0o755), "drwxr-xr-x");
        assert_eq!(render_mode_text(FileKind::File, 0o640), "-rw-r-----");
        assert_eq!(render_mode_text(FileKind::Symlink, 0o777), "lrwxrwxrwx");
        // 未知类型用 ls 的 `?` 前缀，不伪装成普通文件
        assert_eq!(render_mode_text(FileKind::Other, 0o000), "?---------");
        // setuid/setgid/sticky 必须按 ls 规则改写执行位，否则与 `ls -lA` 对不上
        assert_eq!(render_mode_text(FileKind::File, 0o4755), "-rwsr-xr-x");
        assert_eq!(render_mode_text(FileKind::File, 0o2755), "-rwxr-sr-x");
        assert_eq!(render_mode_text(FileKind::Dir, 0o1777), "drwxrwxrwt");
        // 没有执行位时用大写 S/T（ls 同规则）
        assert_eq!(render_mode_text(FileKind::File, 0o4644), "-rwSr--r--");
        // setgid 与 sticky 同时命中无执行位：大写 S/T 各自归位
        assert_eq!(render_mode_text(FileKind::Dir, 0o3666), "drw-rwSrwT");
        // mode 只保留权限位：类型位不得渗进渲染结果
        assert_eq!(
            render_mode_text(FileKind::File, 0o100755 & PERMISSION_BITS),
            "-rwxr-xr-x"
        );
    }

    #[test]
    fn filesystem_list_result_defaults_keep_absent_evidence_out_of_the_wire() {
        let result = FilesystemListResult {
            path: "/data/local/tmp".into(),
            entries: vec![FileStat {
                name: "a b.txt".into(),
                kind: FileKind::File,
                mode: 0o644,
                mode_text: render_mode_text(FileKind::File, 0o644),
                uid: 2000,
                gid: 2000,
                size: 12,
                mtime_unix: 1_760_000_000,
                symlink_target: None,
                readable: true,
            }],
            truncated: false,
            unreadable: vec![],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["entries"][0]["kind"], "file");
        assert_eq!(value["entries"][0]["mode"], json!(420));
        assert_eq!(value["entries"][0]["mtime_unix"], json!(1_760_000_000));
        assert_eq!(value["entries"][0].get("symlink_target"), None);
        assert_eq!(value.get("truncated"), None);
        assert_eq!(value.get("unreadable"), None);

        let parsed: FilesystemListResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.entries[0].name, "a b.txt");
        assert!(!parsed.truncated && parsed.unreadable.is_empty());
    }

    #[test]
    fn preview_params_cap_and_tail_semantics_are_explicit() {
        let params: FilesystemPreviewParams =
            serde_json::from_value(json!({ "path": "/data/local/tmp/x.log" })).unwrap();
        assert_eq!(params.max_bytes, None);
        assert!(!params.from_end);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("max_bytes"), None);
        assert_eq!(value["from_end"], json!(false));

        let result = FilesystemPreviewResult {
            path: "/data/local/tmp/x.log".into(),
            size: 4096,
            offset: 3072,
            returned_bytes: 1024,
            encoding: PreviewEncoding::Hex,
            text: None,
            hex: Some("7f454c46".into()),
            truncated: true,
            detail: Some("binary_detected".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["encoding"], "hex");
        assert_eq!(value.get("text"), None);
        assert_eq!(value["truncated"], json!(true));
        // 旧报文缺 truncated/detail 时必须按「未截断」解析，不能默认成截断
        let legacy: FilesystemPreviewResult = serde_json::from_value(json!({
            "path": "/p", "size": 3, "offset": 0, "returned_bytes": 3,
            "encoding": "utf8", "text": "abc"
        }))
        .unwrap();
        assert!(!legacy.truncated);
        assert_eq!(legacy.detail, None);
    }

    #[test]
    fn kill_params_default_to_term_and_keep_identity_evidence_optional() {
        let params: ProcessKillParams = serde_json::from_value(json!({ "pid": 4321 })).unwrap();
        assert_eq!(params.signal, KillSignal::Term);
        assert!(!params.require_root);
        assert_eq!(params.expected_comm, None);

        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["signal"], "term");
        assert_eq!(value.get("expected_comm"), None);
        assert_eq!(value["require_root"], json!(false));
        assert_eq!(KillSignal::Kill.number(), 9);
        assert_eq!(KillSignal::Term.number(), 15);
    }

    #[test]
    fn kill_result_reports_outcome_without_faking_death() {
        let result = ProcessKillResult {
            pid: 4321,
            signal: KillSignal::Kill,
            outcome: KillOutcome::Signaled,
            comm: Some("toybox".into()),
            cmdline: None,
            uid: Some(2000),
            ran_as_root: false,
            verified_dead: false,
            detail: Some("signal_sent_not_confirmed".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["outcome"], "signaled");
        assert_eq!(value["signal"], "kill");
        assert_eq!(value.get("cmdline"), None);
        // 「没确认到」必须能被上层看见，缺字段解析时按 false 处理而不是 true
        let parsed: ProcessKillResult = serde_json::from_value(json!({
            "pid": 1, "signal": "term", "outcome": "already_gone", "ran_as_root": false
        }))
        .unwrap();
        assert!(!parsed.verified_dead);
        assert_eq!(parsed.outcome, KillOutcome::AlreadyGone);
    }

    #[test]
    fn precondition_failed_code_survives_the_wire() {
        let error = AgentError::new(ErrorCode::PreconditionFailed, "身份不匹配");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], "precondition_failed");
        let parsed: AgentError = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.code, ErrorCode::PreconditionFailed);
    }

    #[test]
    fn process_ports_result_distinguishes_empty_from_unreadable() {
        let result = ProcessPortsResult {
            pid: 4321,
            comm: Some("com.target.app".into()),
            cmdline: None,
            ports: vec![ListeningPort {
                port: 11501,
                address: "127.0.0.1".into(),
                family: SocketFamily::Ipv4,
                state: "listen".into(),
                inode: 4242,
                uid: 0,
            }],
            unreadable: vec!["/proc/net/tcp6".into()],
            truncated: false,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["ports"][0]["family"], "ipv4");
        assert_eq!(value["ports"][0]["port"], 11501);
        assert_eq!(value.get("cmdline"), None);
        assert_eq!(value.get("truncated"), None);
        assert_eq!(value["unreadable"][0], "/proc/net/tcp6");
        let parsed: ProcessPortsResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.ports[0].family, SocketFamily::Ipv4);
    }

    #[test]
    fn process_by_port_result_splits_owned_and_unowned_sockets() {
        let socket = PortHoldingProcess {
            pid: 0,
            uid: 0,
            family: SocketFamily::Ipv6,
            address: "::".into(),
            state: "listen".into(),
            inode: 99,
            comm: None,
        };
        let result = ProcessByPortResult {
            port: 8081,
            sockets: vec![socket.clone()],
            unowned: vec![socket],
            truncated: true,
            skipped: vec!["scanned_pid_limit=4000".into()],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["sockets"][0]["family"], "ipv6");
        assert_eq!(value["sockets"][0].get("comm"), None);
        assert_eq!(value["truncated"], json!(true));
        assert_eq!(value["skipped"][0], "scanned_pid_limit=4000");
        // pid=0 表示"socket 存在但属主未知"，不能与"没有该端口"混淆
        let empty: ProcessByPortResult =
            serde_json::from_value(json!({"port": 1, "sockets": []})).unwrap();
        assert!(empty.sockets.is_empty() && empty.unowned.is_empty());
    }

    #[test]
    fn localized_params_accept_missing_locale_as_device_default() {
        let params: PackageListLocalizedParams =
            serde_json::from_value(json!({ "scope": "all", "include_disabled": false })).unwrap();
        assert_eq!(params.locale, None);
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value.get("locale"), None);
    }

    #[test]
    fn localized_channel_is_optional_and_snake_case_compatible() {
        let mut result = PackageListLocalizedResult {
            items: Vec::new(),
            success_count: 0,
            fallback_count: 0,
            warnings: Vec::new(),
            channel: Some("zygisk_v2".into()),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["channel"], "zygisk_v2");
        assert_eq!(value.get("warnings"), None);
        result.channel = None;
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value.get("channel"), None);
        // 旧版 Agent 报文没有该字段，也必须能解出来（不得当成 v2）
        let legacy: PackageListLocalizedResult =
            serde_json::from_value(json!({"items": [], "success_count": 0, "fallback_count": 0}))
                .unwrap();
        assert_eq!(legacy.channel, None);
    }

    #[test]
    fn zygisk_status_keeps_incompatible_flag_optional_on_the_wire() {
        let lean: ZygiskStatusResult = serde_json::from_value(serde_json::json!({
            "lifecycle": "bridge_ready",
            "bridge_ready": true,
            "root_available": false,
            "sub_protocol_version": 2
        }))
        .unwrap();
        assert!(
            !lean.module_incompatible,
            "旧 Agent 不发这个字段时应默认为 false，不能凭空报不兼容"
        );
    }

    #[test]
    fn describe_result_keeps_wire_snake_case_and_optional_fields() {
        let result = PackageDescribeResult {
            package_name: "com.example.app".into(),
            label: "示例应用".into(),
            label_source: LabelSource::Framework,
            requested_locale: "-".into(),
            resolved_locale: Some("zh-Hans-CN".into()),
            fallback_reason: None,
            version_name: Some("1.2.3".into()),
            version_code: Some(7),
            device_locale: Some("zh-Hans-CN".into()),
            is_system: false,
            enabled: true,
            apk_files: vec![
                DescribedApkFile {
                    name: "base.apk".into(),
                    size: 10,
                },
                DescribedApkFile {
                    name: "split_config.arm64_v8a.apk".into(),
                    size: 20,
                },
            ],
        };
        let value = serde_json::to_value(&result).unwrap();
        // 线格式 snake_case（D043：跨 IPC 不加翻译层）
        assert_eq!(value["apk_files"][1]["name"], "split_config.arm64_v8a.apk");
        assert_eq!(value["version_code"], 7);
        assert_eq!(value["labelSource"], serde_json::Value::Null);
        let round: PackageDescribeResult = serde_json::from_value(value).unwrap();
        assert_eq!(round, result);
        // 老模块不回的可选字段缺省时必须能解析
        let lean: PackageDescribeResult = serde_json::from_value(serde_json::json!({
            "package_name": "com.example.app",
            "label": "com.example.app",
            "label_source": "package_name",
            "requested_locale": "-",
            "is_system": false,
            "enabled": true,
            "apk_files": [{"name": "base.apk", "size": 1}]
        }))
        .unwrap();
        assert_eq!(lean.version_name, None);
        assert_eq!(lean.device_locale, None);
    }

    #[test]
    fn staged_export_round_trips_files_and_session() {
        let result = PackageExportApkResult {
            package_name: "com.example.app".into(),
            session: "a1b2c3".into(),
            files: vec![StagedApkFile {
                name: "base.apk".into(),
                size: 1234,
                remote_path: "/data/local/tmp/x/base.apk".into(),
            }],
            bytes: 1234,
            skipped: vec!["split_config.arm64_v8a.apk".into()],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value["files"][0]["remote_path"],
            "/data/local/tmp/x/base.apk"
        );
        assert_eq!(value["files"][0]["name"], "base.apk");
        // 线格式保持 snake_case，不加翻译层（D043）
        assert_eq!(value["skipped"][0], "split_config.arm64_v8a.apk");
        let without: PackageExportApkResult = serde_json::from_value(serde_json::json!({
            "package_name": "com.example.app",
            "session": "a1b2c3",
            "files": [],
            "bytes": 0
        }))
        .unwrap();
        assert!(without.skipped.is_empty(), "旧 Agent 缺字段必须能解析");
    }

    #[test]
    fn device_info_optional_fields_do_not_use_empty_string_sentinels() {
        let result = DeviceInfoResult {
            serial: None,
            model: Some("Pixel Test".into()),
            manufacturer: None,
            android_version: Some("14".into()),
            api_level: Some(34),
            primary_abi: Some("arm64-v8a".into()),
            wlan_ipv4: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["model"], "Pixel Test");
        assert_eq!(value.get("manufacturer"), None);
        assert_eq!(value.get("wlan_ipv4"), None);
    }

    #[test]
    fn capability_list_uses_shared_capability_dto() {
        let result = CapabilityListResult {
            capabilities: vec![CapabilityInfo {
                method: method::SYSTEM_HEALTH.into(),
                version: 1,
                provider: "system".into(),
                available: true,
                unavailable_reason: None,
                probe_pending: false,
            }],
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["capabilities"][0]["method"], "system.health");
        assert_eq!(value["capabilities"][0]["provider"], "system");
    }
}
