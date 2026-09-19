use serde::{Deserialize, Serialize};

use crate::CapabilityInfo;

pub mod method {
    pub const SYSTEM_HELLO: &str = "system.hello";
    pub const SYSTEM_HEALTH: &str = "system.health";
    pub const CAPABILITY_LIST: &str = "capability.list";
    pub const DEVICE_INFO: &str = "device.info";
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_LIST_LOCALIZED: &str = "package.list_localized";
    pub const ACTIVITY_FOREGROUND: &str = "activity.foreground";
    pub const PROCESS_PORTS: &str = "process.ports";
    pub const PROCESS_BY_PORT: &str = "process.by_port";
    pub const PROCESS_KILL: &str = "process.kill";
    pub const FILESYSTEM_LIST: &str = "filesystem.list";
    pub const FILESYSTEM_STAT: &str = "filesystem.stat";
    pub const FILESYSTEM_PREVIEW: &str = "filesystem.preview";
    pub const PACKAGE_EXPORT_APK: &str = "package.export_apk";
    pub const PACKAGE_EXPORT_CLEAN: &str = "package.export_clean";
    pub const ZYGISK_STATUS: &str = "zygisk.status";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_latency_ms: Option<u64>,
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
    /// maps=行数、cmdline=命令行（截断）、status=头几行；不可读为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub readable: bool,
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
    /// 调用方声明这次终止必须 root。Agent 以 shell 身份运行时应显式拒绝，
    /// 不得「试一下失败再说」——那会让 UI 把权限问题当成进程问题。
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
            detail: None,
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["lifecycle"], "installed_reboot_required");
        assert_eq!(value["module_id"], "applist");
        assert_eq!(value["sub_protocol_version"], 1);
        assert_eq!(value.get("detail"), None);
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
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value["files"][0]["remote_path"],
            "/data/local/tmp/x/base.apk"
        );
        assert_eq!(value["files"][0]["name"], "base.apk");
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
            }],
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["capabilities"][0]["method"], "system.health");
        assert_eq!(value["capabilities"][0]["provider"], "system");
    }
}
