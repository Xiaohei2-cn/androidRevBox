//! APK 产物装配：把设备侧取回的 base + split 变成「应用名_版本号」的单个文件。
//!
//! 规则：
//! * 只有一个文件（应用没有分包）→ 直接改名成 `<显示名>_<版本号>.apk`；
//! * 多个文件（base + split_*）→ 装进 **SAI 的 `.apks`**（Split APKs Installer 格式）：
//!   一个 store 打包的 zip，里面是 `meta.sai_v2.json`、`meta.sai_v1.json` 和按名字
//!   排序的 APK。容器内的 APK 名字保持 `base.apk` / `split_*.apk` 原样，安装器按
//!   这些名字识别 split 类型。
//!
//! 为什么是 `.apks` 而不是 `.xapk`（用户口径）：`.apks` 有 SAI 仓库里
//! `META-FORMAT.md` + `ApksSingleBackupTaskExecutor` 这两份可读的权威实现，
//! 字段、条目顺序、打包方式（全 STORED、统一时间戳）都能逐条对齐；`.xapk` 没有
//! 统一规范，各工具的 `manifest.json` 方言互不兼容。
//!
//! 分工：分包**集合**与**按设备语言解析出的应用名/版本号**只能问 Framework，
//! 这是 Zygisk 模块的职责（`E` 导出、`P` 按包查询）；而设备侧 `toybox` 没有 zip
//! 工具，APK 本身又已经压缩过（store 打包即可），文件也必然已经经 ADB pull 回到
//! 电脑，所以「装进容器」这一步放在宿主侧做，既不给设备加负担也好校验回收。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

use crate::core::error::{CoreError, CoreResult};

/// 单个路径分量的字节上限：APFS/ext4 是 255 字节，留出冲突后缀 ` (12)` 的余量。
const MAX_STEM_BYTES: usize = 180;
const MAX_VERSION_BYTES: usize = 60;
const MAX_PART_NAME_BYTES: usize = 200;
/// SAI 的 `.apks` 里两个元数据文件名（逐字取自 SAI 源码常量）
const SAI_META_V2: &str = "meta.sai_v2.json";
const SAI_META_V1: &str = "meta.sai_v1.json";
/// SAI v2 meta 里表示"这个包里装的就是 APK 文件"的组件类型
const SAI_COMPONENT_APK_FILES: &str = "apk_files";
const READ_CHUNK: usize = 64 * 1024;
/// 同一目录内重名时的消歧上限。
const MAX_SUFFIX: u32 = 999;
/// zip 通用标志位 bit 11：文件名按 UTF-8 存。
const FLAG_UTF8_NAMES: u16 = 0x0800;
const METHOD_STORE: u16 = 0;
const VERSION_ZIP_20: u16 = 20;
const LOCAL_HEADER_LEN: u64 = 30;
const CENTRAL_HEADER_LEN: u64 = 46;
const EOCD_LEN: u64 = 22;

/// 装配结果类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind {
    /// 无分包：单个 APK
    Single,
    /// 有分包：SAI 的 .apks 容器
    Bundle,
}

impl BundleKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Single => "apk",
            Self::Bundle => "apks",
        }
    }

    pub fn as_str(self) -> &'static str {
        self.extension()
    }
}

/// 参与命名的应用元数据。由服务层向 Zygisk 清单查询得到，**不接受界面回传的字符串**
/// ——界面上显示的名字可能已经是上一次刷新时的旧值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppNaming {
    pub package_name: String,
    pub label: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    /// `zygisk`：名字与版本来自模块清单；
    /// `fallback_package_name`：清单查询失败或清单里没有这个包。
    pub name_source: String,
}

impl AppNaming {
    /// 命名主串：`<显示名>_<版本号>`（用户要求用下划线，不用空格）；
    /// 版本号缺失时退回 `v<versionCode>`，两者都没有就只有名字。
    pub fn stem(&self) -> String {
        let name = self.display_name();
        let version = sanitize(self.version_name.trim(), MAX_VERSION_BYTES);
        if !version.is_empty() {
            return fit(&format!("{name}_{version}"));
        }
        match self.version_code {
            Some(code) => fit(&format!("{name}_v{code}")),
            None => name,
        }
    }

    fn display_name(&self) -> String {
        let name = sanitize(self.label.trim(), MAX_STEM_BYTES);
        if name.is_empty() {
            sanitize(self.package_name.trim(), MAX_STEM_BYTES)
        } else {
            name
        }
    }
}

/// 已经取回宿主的一个 APK 分片（`name` 是设备侧原名，如 `split_config.arm64_v8a.apk`）。
#[derive(Debug, Clone)]
pub struct PulledPart {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// 装配产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub kind: BundleKind,
    pub file_name: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// 清洗文件名片段：非法字符换成下划线、空白折叠、去首尾点与空格、按字符边界截断。
fn sanitize(raw: &str, max_bytes: usize) -> String {
    const ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut out = String::with_capacity(raw.len().min(max_bytes));
    let mut last_was_space = true;
    for ch in raw.chars() {
        if ch.is_control() || ILLEGAL.contains(&ch) {
            out.push('_');
            last_was_space = false;
        } else if ch.is_whitespace() {
            // 连续空白（含全角空格）压成一个半角空格
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    while out.ends_with([' ', '.']) {
        out.pop();
    }
    while out.starts_with('.') {
        out.remove(0);
    }
    let mut out = truncate_bytes(&out, max_bytes);
    if is_reserved_windows_name(&out) {
        out.insert(0, '_');
        out = truncate_bytes(&out, max_bytes);
    }
    out
}

fn is_reserved_windows_name(name: &str) -> bool {
    let head = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED.contains(&head.as_str())
}

/// 按 UTF-8 字符边界截断，绝不切出半个汉字。
fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_owned()
}

/// 主串拼装后再兜一次长度（名字 + 版本可能一起顶到上限）。
fn fit(value: &str) -> String {
    truncate_bytes(value.trim(), MAX_STEM_BYTES)
}

/// 在同一目录内挑一个不冲突的文件名（`xxx.apk` → `xxx (2).apk`）。
pub async fn reserve_file_name(dir: &Path, stem: &str, ext: &str) -> CoreResult<String> {
    let stem = sanitize(stem, MAX_STEM_BYTES);
    if stem.is_empty() {
        return Err(CoreError::Internal("产物文件名为空".into()));
    }
    for attempt in 0..=MAX_SUFFIX {
        // 与桌面系统的习惯一致：第一个叫 X，撞名后依次是 X (2)、X (3)……
        let candidate = if attempt == 0 {
            format!("{stem}.{ext}")
        } else {
            let suffix = attempt + 1;
            format!("{stem} ({suffix}).{ext}")
        };
        if tokio::fs::try_exists(dir.join(&candidate))
            .await
            .unwrap_or(false)
        {
            continue;
        }
        return Ok(candidate);
    }
    Err(CoreError::Internal(format!(
        "目标目录里同名的产物太多（>{MAX_SUFFIX}）"
    )))
}

/// SAI `.apks` 的 v2 元数据。字段名逐字对齐 SAI 的 `SaiExportedAppMeta2`
/// （`@SerializedName` 那一套），字段顺序无所谓但**缺字段是会被读成 null 的**。
///
/// `min_sdk` / `target_sdk` 在 SAI 里是 `@Nullable`，我们目前从 Framework 拿不到
/// （`P` 命令没这两个字段），所以**刻意不写**——写 0 或猜一个值比留空更糟。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SaiMetaV2 {
    pub meta_version: u32,
    #[serde(rename = "package")]
    pub package: String,
    pub label: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    /// Unix 毫秒，与 SAI 一致
    pub export_timestamp: u64,
    pub split_apk: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_sdk: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sdk: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backup_components: Vec<SaiBackupComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SaiBackupComponent {
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
}

/// SAI `.apks` 的 v1 元数据（`SaiExportedAppMeta`），字段更少，老版本安装器读这份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SaiMetaV1 {
    #[serde(rename = "package")]
    pub package: String,
    pub label: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    pub export_timestamp: u64,
}

/// 生成两份 SAI 元数据（v2、v1）的 JSON 文本。
pub fn build_sai_meta(
    naming: &AppNaming,
    parts: &[PulledPart],
    created: SystemTime,
) -> (String, String) {
    let label = {
        let display = naming.display_name();
        if display.is_empty() {
            naming.package_name.clone()
        } else {
            display
        }
    };
    let timestamp = created
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let total_size = parts.iter().map(|part| part.size).sum::<u64>();
    let v2 = SaiMetaV2 {
        meta_version: 2,
        package: naming.package_name.clone(),
        label: label.clone(),
        version_name: naming.version_name.clone(),
        version_code: naming.version_code,
        export_timestamp: timestamp,
        split_apk: parts.len() > 1,
        min_sdk: None,
        target_sdk: None,
        backup_components: vec![SaiBackupComponent {
            kind: SAI_COMPONENT_APK_FILES.to_owned(),
            size: total_size,
        }],
    };
    let v1 = SaiMetaV1 {
        package: naming.package_name.clone(),
        label,
        version_name: naming.version_name.clone(),
        version_code: naming.version_code,
        export_timestamp: timestamp,
    };
    (to_json(&v2), to_json(&v1))
}

fn to_json<T: Serialize>(value: &T) -> String {
    // 字段全是字符串/整数/数组，理论上不会失败；真失败也不能 panic，给出可诊断的 JSON
    serde_json::to_string(value).unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
}

/// 装配主流程：`parts` 是刚从设备取回、仍留在暂存目录里的原名文件，产物写进
/// `destination`。调用方负责回收暂存目录；本函数失败时不会留下半截产物。
pub async fn assemble(
    naming: &AppNaming,
    parts: Vec<PulledPart>,
    destination: &Path,
) -> CoreResult<Artifact> {
    if parts.is_empty() {
        return Err(CoreError::Internal("没有可装配的 APK 文件".into()));
    }
    let kind = if parts.len() == 1 {
        BundleKind::Single
    } else {
        BundleKind::Bundle
    };
    let file_name = reserve_file_name(destination, &naming.stem(), kind.extension()).await?;
    let target = destination.join(&file_name);

    let written = match kind {
        BundleKind::Single => place_file(&parts[0].path, &target).await,
        BundleKind::Bundle => write_bundle(&target, naming, &parts).await,
    };
    match written {
        Ok(bytes) => Ok(Artifact {
            kind,
            file_name,
            path: target,
            bytes,
        }),
        Err(error) => {
            let _cleanup = tokio::fs::remove_file(&target).await;
            Err(error)
        }
    }
}

/// 单包：优先原地改名（同一卷上零拷贝），跨卷时退回复制。
async fn place_file(source: &Path, target: &Path) -> CoreResult<u64> {
    if tokio::fs::rename(source, target).await.is_err() {
        tokio::fs::copy(source, target)
            .await
            .map_err(|error| CoreError::Internal(format!("写出 APK 失败: {error}")))?;
        let _removed = tokio::fs::remove_file(source).await;
    }
    metadata_size(target).await
}

/// 多包：写一个 store（不压缩）的 zip 容器 = SAI 的 `.apks`。
/// 条目顺序与时间戳照 SAI 自己的写入器（`ApksSingleBackupTaskExecutor`）：
/// `meta.sai_v2.json` → `meta.sai_v1.json` → 按文件名排序的 APK，共用同一个导出时间戳。
async fn write_bundle(target: &Path, naming: &AppNaming, parts: &[PulledPart]) -> CoreResult<u64> {
    let created = SystemTime::now();
    let (meta_v2, meta_v1) = build_sai_meta(naming, parts, created);
    let file = tokio::fs::File::create(target)
        .await
        .map_err(|error| CoreError::Internal(format!("创建 apks 失败: {error}")))?;
    let mut writer = BufWriter::with_capacity(READ_CHUNK, file);
    let mut entries: Vec<ZipEntry> = Vec::with_capacity(parts.len() + 2);
    let now = dos_of(created);

    let mut offset = put_bytes(
        &mut writer,
        &mut entries,
        0,
        SAI_META_V2,
        meta_v2.as_bytes(),
        now,
    )
    .await?;
    offset = put_bytes(
        &mut writer,
        &mut entries,
        offset,
        SAI_META_V1,
        meta_v1.as_bytes(),
        now,
    )
    .await?;
    // SAI 排序后写入；跟着排，两次导出的产物才有可比对性
    let mut ordered: Vec<&PulledPart> = parts.iter().collect();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    for part in ordered {
        let name = sanitize(&part.name, MAX_PART_NAME_BYTES);
        if name.is_empty() {
            return Err(CoreError::Internal("分包名为空".into()));
        }
        // 先预扫一遍算 CRC/长度（APK 都在本地暂存目录，多一次顺序读很便宜），
        // 这样 local header 就能一次写对，不需要回头 seek 修补。
        let meta = scan_file(part).await?;
        offset = put_file(&mut writer, &mut entries, offset, &name, part, meta, now).await?;
    }
    finish_zip(&mut writer, &entries, offset).await?;
    writer
        .shutdown()
        .await
        .map_err(|error| CoreError::Internal(format!("apks 写入未完成: {error}")))?;
    drop(writer);
    // 写完立刻按中央目录读回来对一遍：条目数量、名字、长度、偏移、CRC 全对上才算成功。
    // 只读尾部与中央目录，不重读文件体，代价可忽略；换来的是「安装器一定读得懂这个容器」。
    let read = read_bundle(target).await?;
    if read.len() != entries.len() {
        return Err(CoreError::Internal(format!(
            "容器自检条目数不符：写入 {}，读回 {}",
            entries.len(),
            read.len()
        )));
    }
    for (written, read_back) in entries.iter().zip(read.iter()) {
        if written.name != read_back.name
            || written.size != read_back.size
            || written.crc != read_back.crc
            || written.offset != read_back.offset
        {
            return Err(CoreError::Internal(format!(
                "容器自检不一致：写入 {:?}，读回 {:?}",
                (
                    written.name.as_str(),
                    written.size,
                    written.crc,
                    written.offset
                ),
                (
                    read_back.name.as_str(),
                    read_back.size,
                    read_back.crc,
                    read_back.offset
                )
            )));
        }
    }
    metadata_size(target).await
}

#[derive(Debug, Clone)]
struct ZipEntry {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
    dos_time: u16,
    dos_date: u16,
}

fn u32_field(value: u64, what: &str) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| CoreError::Internal(format!("{what} 超过 4 GiB，zip 记不下")))
}

fn u16_field(value: usize, what: &str) -> CoreResult<u16> {
    u16::try_from(value).map_err(|_| CoreError::Internal(format!("{what} 超出 16 位上限")))
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

async fn put_bytes<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    entries: &mut Vec<ZipEntry>,
    offset: u64,
    name: &str,
    data: &[u8],
    (dos_time, dos_date): (u16, u16),
) -> CoreResult<u64> {
    let crc = crc32(data);
    write_local_header(writer, name, data.len() as u64, crc, dos_time, dos_date).await?;
    writer
        .write_all(data)
        .await
        .map_err(|error| CoreError::Internal(format!("写入 {name} 失败: {error}")))?;
    entries.push(ZipEntry {
        name: name.to_owned(),
        crc,
        size: data.len() as u64,
        offset,
        dos_time,
        dos_date,
    });
    Ok(offset + LOCAL_HEADER_LEN + name.len() as u64 + data.len() as u64)
}

/// 预扫得到的分片元信息（CRC32 与本地实际长度）。
#[derive(Debug, Clone, Copy)]
struct PartMeta {
    crc: u32,
    size: u64,
}

/// 预扫一遍分包：算 CRC32 并核对本地字节数与设备侧声明一致。
async fn scan_file(part: &PulledPart) -> CoreResult<PartMeta> {
    let mut source = tokio::fs::File::open(&part.path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开 {} 失败: {error}", part.name)))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut size = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| CoreError::Internal(format!("读取 {} 失败: {error}", part.name)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    if size != part.size {
        return Err(CoreError::Internal(format!(
            "{} 大小不符：设备侧 {}，本地 {size}",
            part.name, part.size
        )));
    }
    Ok(PartMeta {
        crc: hasher.finalize(),
        size,
    })
}

/// 把分包原样抄进容器（store：APK 本身已压缩，二次压缩既慢又可能变大）。
async fn put_file<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    entries: &mut Vec<ZipEntry>,
    offset: u64,
    name: &str,
    part: &PulledPart,
    meta: PartMeta,
    (dos_time, dos_date): (u16, u16),
) -> CoreResult<u64> {
    let PartMeta { crc, size } = meta;
    write_local_header(writer, name, size, crc, dos_time, dos_date).await?;
    let mut source = tokio::fs::File::open(&part.path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开 {} 失败: {error}", part.name)))?;
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut written = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| CoreError::Internal(format!("读取 {} 失败: {error}", part.name)))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|error| CoreError::Internal(format!("写入 {name} 失败: {error}")))?;
        written += read as u64;
    }
    if written != size {
        return Err(CoreError::Internal(format!(
            "{name} 写入中途变小：计划 {size}，实际 {written}"
        )));
    }
    entries.push(ZipEntry {
        name: name.to_owned(),
        crc,
        size: written,
        offset,
        dos_time,
        dos_date,
    });
    Ok(offset + LOCAL_HEADER_LEN + name.len() as u64 + written)
}

async fn write_local_header<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    name: &str,
    size: u64,
    crc: u32,
    dos_time: u16,
    dos_date: u16,
) -> CoreResult<()> {
    let name_bytes = name.as_bytes();
    let size = u32_field(size, "分包体积")?;
    let name_len = u16_field(name_bytes.len(), "分包名长度")?;
    let mut header = [0_u8; 30];
    header[0..4].copy_from_slice(b"PK\x03\x04");
    header[4..6].copy_from_slice(&VERSION_ZIP_20.to_le_bytes());
    header[6..8].copy_from_slice(&FLAG_UTF8_NAMES.to_le_bytes());
    header[8..10].copy_from_slice(&METHOD_STORE.to_le_bytes());
    header[10..12].copy_from_slice(&dos_time.to_le_bytes());
    header[12..14].copy_from_slice(&dos_date.to_le_bytes());
    header[14..18].copy_from_slice(&crc.to_le_bytes());
    header[18..22].copy_from_slice(&size.to_le_bytes());
    header[22..26].copy_from_slice(&size.to_le_bytes());
    header[26..28].copy_from_slice(&name_len.to_le_bytes());
    header[28..30].copy_from_slice(&0_u16.to_le_bytes());
    writer
        .write_all(&header)
        .await
        .map_err(|error| CoreError::Internal(format!("写 zip 本地头失败: {error}")))?;
    writer
        .write_all(name_bytes)
        .await
        .map_err(|error| CoreError::Internal(format!("写 zip 文件名失败: {error}")))?;
    Ok(())
}

async fn finish_zip<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    entries: &[ZipEntry],
    end_of_data: u64,
) -> CoreResult<()> {
    for entry in entries {
        let name_bytes = entry.name.as_bytes();
        let size = u32_field(entry.size, "分包体积")?;
        let mut header = [0_u8; 46];
        header[0..4].copy_from_slice(b"PK\x01\x02");
        header[4..6].copy_from_slice(&VERSION_ZIP_20.to_le_bytes());
        header[6..8].copy_from_slice(&VERSION_ZIP_20.to_le_bytes());
        header[8..10].copy_from_slice(&FLAG_UTF8_NAMES.to_le_bytes());
        header[10..12].copy_from_slice(&METHOD_STORE.to_le_bytes());
        header[12..14].copy_from_slice(&entry.dos_time.to_le_bytes());
        header[14..16].copy_from_slice(&entry.dos_date.to_le_bytes());
        header[16..20].copy_from_slice(&entry.crc.to_le_bytes());
        header[20..24].copy_from_slice(&size.to_le_bytes());
        header[24..28].copy_from_slice(&size.to_le_bytes());
        header[28..30].copy_from_slice(&u16_field(name_bytes.len(), "分包名长度")?.to_le_bytes());
        header[30..32].copy_from_slice(&0_u16.to_le_bytes());
        header[32..34].copy_from_slice(&0_u16.to_le_bytes());
        header[34..36].copy_from_slice(&0_u16.to_le_bytes());
        header[36..38].copy_from_slice(&0_u16.to_le_bytes());
        // external attrs = 0o644 << 16，表明是普通文件
        header[38..42].copy_from_slice(&(0o644_u32 << 16).to_le_bytes());
        header[42..46].copy_from_slice(&u32_field(entry.offset, "分包偏移")?.to_le_bytes());
        writer
            .write_all(&header)
            .await
            .map_err(|error| CoreError::Internal(format!("写中央目录失败: {error}")))?;
        writer
            .write_all(name_bytes)
            .await
            .map_err(|error| CoreError::Internal(format!("写中央目录文件名失败: {error}")))?;
    }
    let cd_size = entries.len() as u64 * CENTRAL_HEADER_LEN
        + entries
            .iter()
            .map(|entry| entry.name.len() as u64)
            .sum::<u64>();
    let count = entries.len();
    let mut eocd = [0_u8; 22];
    eocd[0..4].copy_from_slice(b"PK\x05\x06");
    eocd[4..6].copy_from_slice(&0_u16.to_le_bytes());
    eocd[6..8].copy_from_slice(&0_u16.to_le_bytes());
    eocd[8..10].copy_from_slice(&u16_field(count, "分包数量")?.to_le_bytes());
    eocd[10..12].copy_from_slice(&u16_field(count, "分包数量")?.to_le_bytes());
    eocd[12..16].copy_from_slice(&u32_field(cd_size, "中央目录长度")?.to_le_bytes());
    eocd[16..20].copy_from_slice(&u32_field(end_of_data, "中央目录偏移")?.to_le_bytes());
    eocd[20..22].copy_from_slice(&0_u16.to_le_bytes());
    writer
        .write_all(&eocd)
        .await
        .map_err(|error| CoreError::Internal(format!("写 zip 结尾失败: {error}")))?;
    Ok(())
}

async fn metadata_size(path: &Path) -> CoreResult<u64> {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .map_err(|error| CoreError::Internal(format!("读取产物大小失败: {error}")))
}

/// `.apks` 容器里的一个条目（只解析中央目录，不预读文件体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleEntry {
    pub name: String,
    pub size: u64,
    pub crc: u32,
    pub offset: u64,
}

/// 解析自己写出去的容器：既是给测试用的“独立读路径”，也给界面上
/// “产物里到底装了哪几个分包”这类检查用。读尾部 EOCD + 中央目录，不整包进内存。
pub async fn read_bundle(path: &Path) -> CoreResult<Vec<BundleEntry>> {
    use tokio::io::AsyncSeekExt;
    const TAIL: u64 = 8 * 1024;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开产物失败: {error}")))?;
    let len = file
        .metadata()
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物信息失败: {error}")))?
        .len();
    if len < EOCD_LEN {
        return Err(CoreError::Internal("产物太小，不是有效的 zip 容器".into()));
    }
    let tail_len = len.min(TAIL);
    let tail_start = len - tail_len;
    file.seek(std::io::SeekFrom::Start(tail_start))
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物尾部失败: {error}")))?;
    let mut tail = vec![0_u8; tail_len as usize];
    file.read_exact(&mut tail)
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物尾部失败: {error}")))?;
    let eocd_at = tail
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .ok_or_else(|| CoreError::Internal("产物缺少 zip 结尾记录".into()))?;
    let read_u16 = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u64;
    let read_u32 = |bytes: &[u8], at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as u64
    };
    let count = read_u16(&tail, eocd_at + 10) as usize;
    let cd_size = read_u32(&tail, eocd_at + 12) as usize;
    let cd_offset = read_u32(&tail, eocd_at + 16) as usize;
    if cd_offset + cd_size + EOCD_LEN as usize != len as usize {
        return Err(CoreError::Internal("容器尾部长度与中央目录不一致".into()));
    }
    file.seek(std::io::SeekFrom::Start(cd_offset as u64))
        .await
        .map_err(|error| CoreError::Internal(format!("读取中央目录失败: {error}")))?;
    let mut cd = vec![0_u8; cd_size];
    file.read_exact(&mut cd)
        .await
        .map_err(|error| CoreError::Internal(format!("读取中央目录失败: {error}")))?;
    let mut entries = Vec::with_capacity(count);
    let mut cursor = 0_usize;
    for _ in 0..count {
        if cd.get(cursor..cursor + 4) != Some(b"PK\x01\x02".as_slice()) {
            return Err(CoreError::Internal("中央目录条目魔数不对".into()));
        }
        let method = read_u16(&cd, cursor + 10);
        if method != METHOD_STORE as u64 {
            return Err(CoreError::Internal("apks 条目应当是 store 打包".into()));
        }
        let crc = read_u32(&cd, cursor + 16) as u32;
        let size = read_u32(&cd, cursor + 24);
        let name_len = read_u16(&cd, cursor + 28) as usize;
        let offset = read_u32(&cd, cursor + 42);
        let name = String::from_utf8(cd[cursor + 46..cursor + 46 + name_len].to_vec())
            .map_err(|_| CoreError::Internal("条目名不是合法 UTF-8".into()))?;
        entries.push(BundleEntry {
            name,
            size,
            crc,
            offset,
        });
        cursor += CENTRAL_HEADER_LEN as usize + name_len;
    }
    if cursor != cd_size {
        return Err(CoreError::Internal("中央目录条目长度不符".into()));
    }
    Ok(entries)
}

/// 是不是一个 `.apks` 容器（SAI 的后缀约定，大小写不敏感）。
pub fn is_bundle_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("apks"))
}

/// 宿主侧解包容器用的工作目录前缀（回收与陈旧清扫都按这个名字认）。
pub const WORKDIR_PREFIX: &str = "app-reverse-tools-apks-";
/// 解出来的临时目录超过这个年龄才允许被清扫。
///
/// 门槛故意放得很高（3 天）：这条只是"进程被强杀/系统崩溃"的兜底，
/// 而一次真机安装（176 MB 走 USB）实际只需几十秒。阈值一旦接近真实时长，
/// 清扫就会去删**别的任务正在用的目录**——单测并行跑的时候就这么翻过一次车。
pub const WORKDIR_MAX_AGE_DAYS: u64 = 3;

/// 新建一次解包用的临时目录。每次调用都换名，避免两次安装互相覆盖。
///
/// 顺手先扫一遍陈旧目录：正式回收点在任务收尾（`CommandSpec::cleanup_paths`），
/// 但进程被强杀 / 系统崩溃时那条路不会执行，一包 176 MB 留在临时目录里没人管。
pub async fn create_workdir() -> CoreResult<PathBuf> {
    sweep_stale_workdirs(SystemTime::now()).await;
    let base = std::env::temp_dir().join(format!("{WORKDIR_PREFIX}{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&base)
        .await
        .map_err(|error| CoreError::Internal(format!("创建解包目录失败: {error}")))?;
    Ok(base)
}

/// 清扫陈旧解包目录：只按前缀认，别的目录一概不碰；失败只记日志。
///
/// 存在的理由：临时目录的正式回收点在任务收尾（`CommandSpec::cleanup_paths`），
/// 但进程被强杀、系统崩溃时那条路不会执行，176 MB 一包留在 /tmp 里没人回收。
pub async fn sweep_stale_workdirs(now: SystemTime) {
    let root = std::env::temp_dir();
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return;
    };
    let limit = std::time::Duration::from_secs(WORKDIR_MAX_AGE_DAYS * 24 * 60 * 60);
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(WORKDIR_PREFIX) {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let fresh = metadata
            .modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_none_or(|age| age < limit);
        if fresh {
            continue;
        }
        match tokio::fs::remove_dir_all(entry.path()).await {
            Ok(()) => tracing::info!(path = %entry.path().display(), "清扫陈旧解包目录"),
            Err(error) => tracing::debug!(error = %error, "陈旧解包目录清理失败（忽略）"),
        }
    }
}

/// 容器条目名的安全校验（zip-slip 防线）。
///
/// 我们自己写的容器名字都是干净的，但**解包入口必须假设盒子来自别人**：
/// 一个条目叫 `../../evil.apk` 就能把解包变成任意路径写入。
/// 规则：必须是纯文件名（无任何路径分隔、不能是 `.`/`..` 开头、不含控制字符），
/// 且只允许字母数字与 `. _ - +`。
fn safe_entry_name(name: &str) -> CoreResult<&str> {
    let ok = !name.is_empty()
        && name.len() <= MAX_PART_NAME_BYTES
        && !name.contains('/')
        && !name.contains('\\')
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') || c == ' ');
    if ok {
        Ok(name)
    } else {
        Err(CoreError::Internal(format!(
            "容器里有不安全的条目名，拒绝解包: {name:?}"
        )))
    }
}

/// 把 `.apks` 容器解到 `workdir`，返回其中**可用于安装的 APK 路径**（按名字排序）。
///
/// 三条硬规则，都是为了"要么装得上、要么明确说清为什么装不上"：
/// 1. 只取 `.apk` 结尾的条目（与 SAI 自己的 `ZipApkSource` 同一口径，大小写不敏感），
///    所以两份 meta 与可能存在的 `icon.png` 不会被当成安装包；
/// 2. 每个条目边写边算 CRC，与中央目录不一致就**整体失败并删掉解包目录**——
///    半个 base.apk 交给 adb 只会换来一句含义模糊的解析错误；
/// 3. 条目名过 `safe_entry_name`，防 zip-slip。
pub async fn extract_bundle(source: &Path, workdir: &Path) -> CoreResult<Vec<String>> {
    let entries = read_bundle(source).await?;
    let mut apks: Vec<&BundleEntry> = entries
        .iter()
        .filter(|entry| {
            entry
                .name
                .rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("apk"))
        })
        .collect();
    if apks.is_empty() {
        return Err(CoreError::Internal(format!(
            "{} 里没有任何 .apk 条目，不是可安装的容器",
            source.display()
        )));
    }
    for entry in &apks {
        safe_entry_name(&entry.name)?;
        if entry.size == 0 {
            return Err(CoreError::Internal(format!("{} 是空文件", entry.name)));
        }
    }
    apks.sort_by(|left, right| left.name.cmp(&right.name));

    let mut out = Vec::with_capacity(apks.len());
    for entry in apks {
        let target = workdir.join(&entry.name);
        if let Err(error) = stream_entry_to_file(source, entry, &target).await {
            let _cleanup = tokio::fs::remove_dir_all(workdir).await;
            return Err(error);
        }
        out.push(target.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// 从容器里把某个条目原样抄到本地文件，同时核对长度与 CRC。
async fn stream_entry_to_file(source: &Path, entry: &BundleEntry, target: &Path) -> CoreResult<()> {
    use tokio::io::AsyncSeekExt;
    let mut from = tokio::fs::File::open(source)
        .await
        .map_err(|error| CoreError::Internal(format!("打开容器失败: {error}")))?;
    let mut header = [0_u8; LOCAL_HEADER_LEN as usize];
    from.seek(std::io::SeekFrom::Start(entry.offset))
        .await
        .map_err(|error| CoreError::Internal(format!("定位条目失败: {error}")))?;
    from.read_exact(&mut header)
        .await
        .map_err(|error| CoreError::Internal(format!("读条目头失败: {error}")))?;
    if &header[0..4] != b"PK\x03\x04" {
        return Err(CoreError::Internal(format!("{} 的本地头不对", entry.name)));
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let stored_crc = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
    let stored_size = u32::from_le_bytes([header[18], header[19], header[20], header[21]]) as u64;
    if stored_crc != entry.crc || stored_size != entry.size {
        // 本地头与中央目录互相矛盾：容器被改过，不能信任何一条
        return Err(CoreError::Internal(format!(
            "{} 的两处头部不一致（本地 {:08x}/{size}，中央 {:08x}/{})",
            entry.name,
            stored_crc,
            entry.crc,
            entry.size,
            size = stored_size
        )));
    }
    from.seek(std::io::SeekFrom::Start(
        entry.offset + LOCAL_HEADER_LEN + name_len,
    ))
    .await
    .map_err(|error| CoreError::Internal(format!("定位条目数据失败: {error}")))?;

    let mut to = tokio::fs::File::create(target)
        .await
        .map_err(|error| CoreError::Internal(format!("写解包文件失败: {error}")))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut left = entry.size;
    while left > 0 {
        let want = (left as usize).min(READ_CHUNK);
        let read = from
            .read(&mut buffer[..want])
            .await
            .map_err(|error| CoreError::Internal(format!("读容器失败: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        to.write_all(&buffer[..read])
            .await
            .map_err(|error| CoreError::Internal(format!("写解包文件失败: {error}")))?;
        left -= read as u64;
    }
    to.flush().await.ok();
    if left != 0 {
        return Err(CoreError::Internal(format!(
            "{} 只有 {} 字节可读，容器声明 {} 字节",
            entry.name,
            entry.size - left,
            entry.size
        )));
    }
    if hasher.finalize() != entry.crc {
        return Err(CoreError::Internal(format!(
            "{} 的 CRC 与容器声明不一致，包已被改动或损坏",
            entry.name
        )));
    }
    Ok(())
}

/// 读出容器里某个条目的原始字节（自检与测试用：确认分包内容一个字节都没变）。
#[cfg(test)]
pub async fn bundle_payload(path: &Path, entry: &BundleEntry) -> CoreResult<Vec<u8>> {
    use tokio::io::AsyncSeekExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| CoreError::Internal(format!("打开产物失败: {error}")))?;
    let mut header = [0_u8; LOCAL_HEADER_LEN as usize];
    file.seek(std::io::SeekFrom::Start(entry.offset))
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物失败: {error}")))?;
    file.read_exact(&mut header)
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物失败: {error}")))?;
    if &header[0..4] != b"PK\x03\x04" {
        return Err(CoreError::Internal("本地头魔数不对".into()));
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let mut payload = vec![0_u8; entry.size as usize];
    file.seek(std::io::SeekFrom::Start(
        entry.offset + LOCAL_HEADER_LEN + name_len,
    ))
    .await
    .map_err(|error| CoreError::Internal(format!("读取产物失败: {error}")))?;
    file.read_exact(&mut payload)
        .await
        .map_err(|error| CoreError::Internal(format!("读取产物失败: {error}")))?;
    Ok(payload)
}

/// SystemTime → DOS time/date（UTC）。1980 之前夹到 1980-01-01，2108 之前够用。
fn dos_of(at: SystemTime) -> (u16, u16) {
    let secs = unix_secs(at);
    let (year, month, day, hour, minute, second) = civil(secs);
    let year = year.clamp(1980, 2107) as u16;
    let date = (year - 1980) << 9 | (month as u16) << 5 | (day as u16);
    let time = (hour as u16) << 11 | (minute as u16) << 5 | ((second as u16) / 2);
    (time, date)
}

fn unix_secs(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    }
}

/// Howard Hinnant 的 days→civil（UTC）：不额外拉时区库。
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rest / 3_600) as u32,
        (rest % 3_600 / 60) as u32,
        (rest % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn naming(label: &str, version: &str) -> AppNaming {
        AppNaming {
            package_name: "com.example.app".into(),
            label: label.into(),
            version_name: version.into(),
            version_code: Some(42),
            name_source: "zygisk".into(),
        }
    }

    #[test]
    fn stem_joins_label_and_version_with_underscore() {
        // 用户口径：名字与版本号之间用下划线，不用空格
        assert_eq!(
            naming("亚马逊购物", "32.17.0.100").stem(),
            "亚马逊购物_32.17.0.100"
        );
        // 版本号缺失时退回 versionCode，两者都没有就只留名字
        assert_eq!(naming("设置", "").stem(), "设置_v42");
        let mut no_code = naming("设置", "");
        no_code.version_code = None;
        assert_eq!(no_code.stem(), "设置");
        // 名字解析不出来（清单回退成包名）时用包名，不产出空文件名
        assert_eq!(naming("  ", "1.0").stem(), "com.example.app_1.0");
    }

    #[test]
    fn sanitize_strips_path_and_illegal_characters() {
        assert_eq!(sanitize("../../evil", 180), "_.._evil");
        assert_eq!(sanitize("a/b:c*d?e|f\"g<h>i", 180), "a_b_c_d_e_f_g_h_i");
        assert_eq!(sanitize("  很   多   空白  ", 180), "很 多 空白");
        assert_eq!(sanitize("尾部点与空格...  ", 180), "尾部点与空格");
        assert_eq!(sanitize("CON", 180), "_CON");
        assert_eq!(sanitize("nul.txt", 180), "_nul.txt");
        assert_eq!(sanitize("换行\n与\t制表", 180), "换行_与_制表");
        // 截断必须落在字符边界上
        let long = "汉字".repeat(200);
        let cut = sanitize(&long, 10);
        assert_eq!(cut, "汉字汉");
        assert!(cut.len() <= 10);
    }

    /// 两份 SAI 元数据的字段名必须逐字对齐 SAI 源码里的 @SerializedName——
    /// 拼错的后果是"安装器读得到 APK、却显示不出应用信息"，界面上很难发现。
    #[test]
    fn sai_meta_files_match_the_upstream_field_names() {
        let parts = vec![
            PulledPart {
                name: "base.apk".into(),
                path: "/tmp/base.apk".into(),
                size: 10,
            },
            PulledPart {
                name: "split_config.arm64_v8a.apk".into(),
                path: "/tmp/split_config.arm64_v8a.apk".into(),
                size: 5,
            },
        ];
        let (v2_text, v1_text) = build_sai_meta(
            &naming("亚马逊购物", "32.17.0.100"),
            &parts,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_790_059_800_123),
        );
        let v2: HashMap<String, serde_json::Value> = serde_json::from_str(&v2_text).unwrap();
        let v1: HashMap<String, serde_json::Value> = serde_json::from_str(&v1_text).unwrap();

        assert_eq!(v2["meta_version"], 2);
        assert_eq!(v2["package"], "com.example.app");
        assert_eq!(v2["label"], "亚马逊购物");
        assert_eq!(v2["version_name"], "32.17.0.100");
        assert_eq!(v2["version_code"], 42);
        assert_eq!(v2["export_timestamp"], 1_790_059_800_123_u64);
        assert_eq!(v2["split_apk"], true);
        assert_eq!(v2["backup_components"][0]["type"], "apk_files");
        assert_eq!(v2["backup_components"][0]["size"], 15);
        // 拿不到的字段宁可不写，也不填 0 假装知道
        assert_eq!(v2.get("min_sdk"), None);
        assert_eq!(v2.get("target_sdk"), None);

        assert_eq!(v1["package"], "com.example.app");
        assert_eq!(v1["label"], "亚马逊购物");
        assert_eq!(v1["version_name"], "32.17.0.100");
        assert_eq!(v1["version_code"], 42);
        assert_eq!(v1["export_timestamp"], 1_790_059_800_123_u64);
        assert_eq!(v1.get("split_apk"), None, "v1 不该出现 v2 才有的字段");

        // 只有一个分片时 split_apk 必须是 false
        let (single_v2, _) =
            build_sai_meta(&naming("设置", "1"), &parts[..1], SystemTime::UNIX_EPOCH);
        let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(&single_v2).unwrap();
        assert_eq!(parsed["split_apk"], false);
    }

    #[test]
    fn dos_and_civil_conversion_are_correct() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_790_059_800), (2026, 9, 22, 6, 50, 0));
        // 1980-01-01 是 DOS 能表达的最小日期，早于它的时间戳被夹到这里
        assert_eq!(dos_of(SystemTime::UNIX_EPOCH).1, 33);
        let (time, date) =
            dos_of(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_059_800));
        assert_eq!(date, ((2026 - 1980) << 9) | (9 << 5) | 22);
        assert_eq!(time, (6 << 11) | (50 << 5));
    }

    #[tokio::test]
    async fn reserve_name_deduplicates_in_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            reserve_file_name(dir.path(), "亚马逊购物_1.0", "apk")
                .await
                .unwrap(),
            "亚马逊购物_1.0.apk"
        );
        tokio::fs::write(dir.path().join("亚马逊购物_1.0.apk"), b"x")
            .await
            .unwrap();
        assert_eq!(
            reserve_file_name(dir.path(), "亚马逊购物_1.0", "apk")
                .await
                .unwrap(),
            "亚马逊购物_1.0 (2).apk"
        );
        assert!(reserve_file_name(dir.path(), "  ", "apk").await.is_err());
    }

    /// 结构自检：条目顺序、store、CRC、以及“写完再读回来”的完整闭环。
    async fn entries_of(path: &Path) -> Vec<(String, Vec<u8>)> {
        let read = read_bundle(path).await.unwrap();
        let mut out = Vec::new();
        for entry in read {
            let payload = bundle_payload(path, &entry).await.unwrap();
            assert_eq!(payload.len() as u64, entry.size, "{} 长度不符", entry.name);
            assert_eq!(crc32(&payload), entry.crc, "{} 的 CRC 不符", entry.name);
            out.push((entry.name, payload));
        }
        out
    }

    #[tokio::test]
    async fn bundle_keeps_apk_bytes_and_names_and_manifest() {
        let root = tempfile::tempdir().unwrap();
        let stage = root.path().join("stage");
        let out = root.path().join("out");
        tokio::fs::create_dir_all(&stage).await.unwrap();
        tokio::fs::create_dir_all(&out).await.unwrap();

        let base = b"PK\x03\x04base-apk-bytes\x00\x01".to_vec();
        let split = b"split-config-bytes".to_vec();
        tokio::fs::write(stage.join("base.apk"), &base)
            .await
            .unwrap();
        tokio::fs::write(stage.join("split_config.zh.apk"), &split)
            .await
            .unwrap();
        let parts = vec![
            PulledPart {
                name: "base.apk".into(),
                path: stage.join("base.apk"),
                size: base.len() as u64,
            },
            PulledPart {
                name: "split_config.zh.apk".into(),
                path: stage.join("split_config.zh.apk"),
                size: split.len() as u64,
            },
        ];
        let artifact = assemble(&naming("亚马逊购物", "32.17"), parts, &out)
            .await
            .unwrap();
        assert_eq!(artifact.kind, BundleKind::Bundle);
        assert_eq!(artifact.file_name, "亚马逊购物_32.17.apks");
        assert!(artifact.path.starts_with(&out));
        assert!(artifact.bytes > base.len() as u64 + split.len() as u64);
        // 条目顺序照 SAI：两份 meta，然后按文件名排序的 APK
        let entries = entries_of(&artifact.path).await;
        assert_eq!(
            entries
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "meta.sai_v2.json",
                "meta.sai_v1.json",
                "base.apk",
                "split_config.zh.apk"
            ]
        );
        assert_eq!(entries[2].1, base, "APK 字节必须原样保留");
        assert_eq!(entries[3].1, split);
        // SAI 的安装路径只挑 .apk 结尾的条目，meta 放前面不影响它；
        // 但 meta 内容决定它在备份列表里显示成什么
        let meta: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&entries[0].1).unwrap();
        assert_eq!(
            meta["backup_components"][0]["size"],
            base.len() as u64 + split.len() as u64
        );
        assert_eq!(meta["split_apk"], true);
        assert_eq!(meta["label"], "亚马逊购物");
        let v1: HashMap<String, serde_json::Value> = serde_json::from_slice(&entries[1].1).unwrap();
        assert_eq!(v1["package"], "com.example.app");
    }

    #[tokio::test]
    async fn single_apk_is_renamed_without_rewriting_bytes() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("out");
        tokio::fs::create_dir_all(&out).await.unwrap();
        let staged = out.join("stage.apk");
        let payload = b"PK\x03\x04only-one".to_vec();
        tokio::fs::write(&staged, &payload).await.unwrap();
        let artifact = assemble(
            &naming("剪贴岛", "1.2.3"),
            vec![PulledPart {
                name: "base.apk".into(),
                path: staged.clone(),
                size: payload.len() as u64,
            }],
            &out,
        )
        .await
        .unwrap();
        assert_eq!(artifact.kind, BundleKind::Single);
        assert_eq!(artifact.file_name, "剪贴岛_1.2.3.apk");
        assert_eq!(tokio::fs::read(&artifact.path).await.unwrap(), payload);
        assert!(
            !tokio::fs::try_exists(&staged).await.unwrap(),
            "改名后不该留下暂存副本"
        );
    }

    #[tokio::test]
    async fn size_mismatch_aborts_and_leaves_no_partial_artifact() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("out");
        tokio::fs::create_dir_all(&out).await.unwrap();
        let one = out.join("p1.apk");
        let two = out.join("p2.apk");
        tokio::fs::write(&one, b"aaaa").await.unwrap();
        tokio::fs::write(&two, b"bbbb").await.unwrap();
        let error = assemble(
            &naming("测试", "1"),
            vec![
                PulledPart {
                    name: "base.apk".into(),
                    path: one,
                    size: 4,
                },
                PulledPart {
                    name: "split_x.apk".into(),
                    path: two,
                    size: 999,
                },
            ],
            &out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("大小不符"), "{error}");
        let mut names = tokio::fs::read_dir(&out).await.unwrap();
        while let Some(entry) = names.next_entry().await.unwrap() {
            assert!(
                entry.file_name().to_string_lossy().ends_with(".apk"),
                "失败后不该留下半成品: {:?}",
                entry.file_name()
            );
        }
    }

    fn part(name: &str, path: PathBuf, size: u64) -> PulledPart {
        PulledPart {
            name: name.to_owned(),
            path,
            size,
        }
    }

    /// 造一个容器供解包测试用：返回 (.apks 路径, 各条目的原始字节)
    async fn make_bundle(dir: &Path, files: &[(&str, Vec<u8>)], stem: &str) -> PathBuf {
        let stage = dir.join("stage");
        tokio::fs::create_dir_all(&stage).await.unwrap();
        let parts: Vec<PulledPart> = files
            .iter()
            .map(|(name, bytes)| {
                let path = stage.join(name);
                std::fs::write(&path, bytes).unwrap();
                part(name, path, bytes.len() as u64)
            })
            .collect();
        let total: u64 = parts.iter().map(|item| item.size).sum();
        let artifact = assemble(&naming(stem, "1.0"), parts, dir).await.unwrap();
        assert_eq!(artifact.kind, BundleKind::Bundle);
        assert!(total > 0);
        artifact.path
    }

    #[tokio::test]
    async fn extract_bundle_round_trips_apks_only() {
        let dir = tempfile::tempdir().unwrap();
        let base = b"PK\x03\x04base-bytes".to_vec();
        let split = b"PK\x03\x04split-bytes-longer".to_vec();
        let bundle = make_bundle(
            dir.path(),
            &[
                ("base.apk", base.clone()),
                ("split_config.zh.apk", split.clone()),
            ],
            "亚马逊购物",
        )
        .await;

        let workdir = dir.path().join("out");
        tokio::fs::create_dir_all(&workdir).await.unwrap();
        let extracted = extract_bundle(&bundle, &workdir).await.unwrap();

        // 两份 SAI meta 不是安装包：只有 .apk 条目会被解出来
        assert_eq!(
            extracted
                .iter()
                .map(|p| Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned())
                .collect::<Vec<_>>(),
            vec!["base.apk", "split_config.zh.apk"]
        );
        assert_eq!(std::fs::read(&extracted[0]).unwrap(), base);
        assert_eq!(std::fs::read(&extracted[1]).unwrap(), split);
        // 扩展名大小写不敏感（与 SAI 的 endsWith 口径一致）
        let bundle2 = make_bundle(
            &dir.path().join("upper"),
            &[("base.apk", base.clone()), ("SPLIT_C.APk", split.clone())],
            "大小写",
        )
        .await;
        let workdir2 = dir.path().join("out2");
        tokio::fs::create_dir_all(&workdir2).await.unwrap();
        let names: Vec<String> = extract_bundle(&bundle2, &workdir2)
            .await
            .unwrap()
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["SPLIT_C.APk", "base.apk"], "按名字排序");
    }

    #[tokio::test]
    async fn extract_bundle_rejects_corrupt_and_useless_containers() {
        let dir = tempfile::tempdir().unwrap();
        let payload = vec![0xa5_u8; 4096];

        // 1) 容器里一个 APK 都没有 -> 明确报错，不返回空列表让上层去猜
        let no_apk = make_bundle(
            &dir.path().join("noapk"),
            &[
                ("readme.txt", payload.clone()),
                ("notes.txt", payload.clone()),
            ],
            "没有apk",
        )
        .await;
        let workdir = dir.path().join("w1");
        tokio::fs::create_dir_all(&workdir).await.unwrap();
        let error = extract_bundle(&no_apk, &workdir).await.unwrap_err();
        assert!(error.to_string().contains(".apk"), "{error}");

        // 2) 数据被改过一个字节 -> CRC 不符，整体失败并清掉已经解出来的部分
        let good = make_bundle(
            &dir.path().join("corrupt"),
            &[
                ("base.apk", payload.clone()),
                ("split_x.apk", payload.clone()),
            ],
            "坏包",
        )
        .await;
        let mut bytes = std::fs::read(&good).unwrap();
        let entries = read_bundle(&good).await.unwrap();
        let target = entries.iter().find(|e| e.name == "base.apk").unwrap();
        let data_at = target.offset as usize + 30 + target.name.len() + 10;
        bytes[data_at] ^= 0xff;
        std::fs::write(&good, &bytes).unwrap();
        let workdir2 = dir.path().join("w2");
        tokio::fs::create_dir_all(&workdir2).await.unwrap();
        let error = extract_bundle(&good, &workdir2).await.unwrap_err();
        assert!(error.to_string().contains("CRC"), "{error}");
        assert!(
            !workdir2.exists(),
            "失败后不该留下半个解包目录: {:?}",
            std::fs::read_dir(&workdir2).map(|mut r| r.next().map(|e| e.map(|e| e.file_name())))
        );
    }

    #[test]
    fn safe_entry_name_blocks_zip_slip() {
        // 盒子可能来自任何人，条目名必须当成不可信输入
        for bad in [
            "../evil.apk",
            "../../etc/x.apk",
            "a/b.apk",
            "a\\b.apk",
            "/abs.apk",
            ".hidden.apk",
            "",
            "..",
            "中文名.apk",
            "a\u{0}b.apk",
        ] {
            assert!(safe_entry_name(bad).is_err(), "{bad:?} 必须被拒");
        }
        for ok in [
            "base.apk",
            "split_config.zh.apk",
            "SPLIT_C.APk",
            "a-b_1.2+3.apk",
        ] {
            assert_eq!(safe_entry_name(ok).unwrap(), ok);
        }
    }

    #[tokio::test]
    async fn stale_workdir_sweep_only_touches_our_old_dirs() {
        // 两条边界都要钉住：①自家但很新的目录不能动（可能正被别的任务用着）；
        // ②只有带我们前缀的目录才允许被扫，别人家的一概不碰。
        let root = std::env::temp_dir();
        let mine = root.join(format!("{}sweep-case", WORKDIR_PREFIX));
        let other = root.join("definitely-not-ours-sweep");
        for dir in [&mine, &other] {
            tokio::fs::create_dir_all(dir).await.unwrap();
            tokio::fs::write(dir.join("x"), b"y").await.unwrap();
        }

        // ① 现在 = 真实时间：两个目录都是刚建的，一个都不该消失
        sweep_stale_workdirs(SystemTime::now()).await;
        assert!(mine.exists(), "新目录被误删，正在跑的安装会被抽掉文件");
        assert!(other.exists(), "前缀不匹配的目录一律不碰");

        // ② 把"现在"推到 4 天之后：只有自家那个消失
        let future = SystemTime::now() + std::time::Duration::from_secs(4 * 24 * 60 * 60);
        sweep_stale_workdirs(future).await;
        assert!(!mine.exists(), "超过阈值的自家残留目录应被回收");
        assert!(other.exists(), "清扫不能越界删别人的目录");
        let _cleanup = tokio::fs::remove_dir_all(&other).await;
    }

    #[test]
    fn is_bundle_path_follows_the_extension() {
        assert!(is_bundle_path("/tmp/亚马逊购物_1.0.apks"));
        assert!(is_bundle_path("/tmp/A.APKS"), "大小写不敏感");
        assert!(!is_bundle_path("/tmp/a.apk"));
        assert!(!is_bundle_path("/tmp/apks"));
        assert!(!is_bundle_path("/tmp/a.zip"));
    }

    #[tokio::test]
    async fn empty_parts_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            assemble(&naming("x", "1"), vec![], root.path())
                .await
                .is_err()
        );
    }
}
