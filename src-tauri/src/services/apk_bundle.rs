//! APK 产物装配：把设备侧取回的 base + split 变成「应用名 + 版本号」的单个文件。
//!
//! 规则：
//! * 只有一个文件（应用没有分包）→ 直接改名成 `<显示名> <版本号>.apk`；
//! * 多个文件（base + split_*）→ 不压缩地装进 zip 容器并补一份 `manifest.json`，
//!   即 `<显示名> <版本号>.xapk`；容器内的名字保持 `base.apk` / `split_*.apk` 原样，
//!   因为安装器按这些名字识别 split 类型。
//!
//! 分工：分包**集合**与**按设备语言解析出的应用名/版本号**只能问 Framework，
//! 这是 Zygisk 模块的职责（`E` 导出、`L` 本地化清单）；而设备侧 `toybox` 没有 zip
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
    /// 有分包：XAPK 容器
    Bundle,
}

impl BundleKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Single => "apk",
            Self::Bundle => "xapk",
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
    /// 命名主串：`<显示名> <版本号>`；版本号缺失时退回 versionCode，两者都没有就只有名字。
    pub fn stem(&self) -> String {
        let name = self.display_name();
        let version = sanitize(self.version_name.trim(), MAX_VERSION_BYTES);
        if !version.is_empty() {
            return fit(&format!("{name} {version}"));
        }
        match self.version_code {
            Some(code) => fit(&format!("{name} v{code}")),
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

/// XAPK 的 `manifest.json`（AndroidFileHost v2 方言；顶层再冗余一份 `file_paths`，
/// 两种读法的安装器都能拿到分包列表）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct XapkManifest {
    pub manifest_version: u32,
    pub name: String,
    pub package_name: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    pub distribution_content_type: String,
    /// UTC，秒级，无时区后缀（与常见 XAPK 写法一致）
    pub date_created: String,
    pub total_size: u64,
    pub file_paths: Vec<String>,
    pub app_list: Vec<XapkManifestApp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct XapkManifestApp {
    pub name: String,
    pub package_name: String,
    pub version_name: String,
    pub version_code: Option<u64>,
    pub file_paths: Vec<String>,
}

pub fn build_manifest(naming: &AppNaming, parts: &[PulledPart], created: SystemTime) -> String {
    let display = naming.display_name();
    let file_paths: Vec<String> = parts
        .iter()
        .map(|part| sanitize(&part.name, MAX_PART_NAME_BYTES))
        .collect();
    let manifest = XapkManifest {
        manifest_version: 2,
        name: display.clone(),
        package_name: naming.package_name.clone(),
        version_name: naming.version_name.clone(),
        version_code: naming.version_code,
        distribution_content_type: "application/vnd.android.package-archive".into(),
        date_created: iso_utc(created),
        total_size: parts.iter().map(|part| part.size).sum::<u64>(),
        file_paths: file_paths.clone(),
        app_list: vec![XapkManifestApp {
            name: display,
            package_name: naming.package_name.clone(),
            version_name: naming.version_name.clone(),
            version_code: naming.version_code,
            file_paths,
        }],
    };
    serde_json::to_string_pretty(&manifest)
        .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}\n"))
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
        BundleKind::Bundle => {
            let manifest = build_manifest(naming, &parts, SystemTime::now());
            write_bundle(&target, manifest.as_bytes(), &parts).await
        }
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

/// 多包：写一个 store（不压缩）的 zip 容器 = XAPK。
async fn write_bundle(target: &Path, manifest: &[u8], parts: &[PulledPart]) -> CoreResult<u64> {
    let file = tokio::fs::File::create(target)
        .await
        .map_err(|error| CoreError::Internal(format!("创建 XAPK 失败: {error}")))?;
    let mut writer = BufWriter::with_capacity(READ_CHUNK, file);
    let mut entries: Vec<ZipEntry> = Vec::with_capacity(parts.len() + 1);
    let now = dos_of(SystemTime::now());

    let mut offset =
        put_bytes(&mut writer, &mut entries, 0, "manifest.json", manifest, now).await?;
    for part in parts {
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
        .map_err(|error| CoreError::Internal(format!("XAPK 写入未完成: {error}")))?;
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

/// XAPK 容器里的一个条目（只解析中央目录，不预读文件体）。
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
            return Err(CoreError::Internal("XAPK 条目应当是 store 打包".into()));
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

fn iso_utc(at: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = civil(unix_secs(at));
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
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
    fn stem_uses_localized_label_and_version() {
        assert_eq!(
            naming("亚马逊购物", "32.17.0.100").stem(),
            "亚马逊购物 32.17.0.100"
        );
        // 版本号缺失时退回 versionCode，两者都没有就只留名字
        assert_eq!(naming("设置", "").stem(), "设置 v42");
        let mut no_code = naming("设置", "");
        no_code.version_code = None;
        assert_eq!(no_code.stem(), "设置");
        // 名字解析不出来（清单回退成包名）时用包名，不产出空文件名
        assert_eq!(naming("  ", "1.0").stem(), "com.example.app 1.0");
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

    #[test]
    fn manifest_has_both_file_path_dialects() {
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
        let json: HashMap<String, serde_json::Value> = serde_json::from_str(&build_manifest(
            &naming("亚马逊购物", "32.17.0.100"),
            &parts,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_059_800),
        ))
        .unwrap();
        assert_eq!(json["manifest_version"], 2);
        assert_eq!(json["name"], "亚马逊购物");
        assert_eq!(json["package_name"], "com.example.app");
        assert_eq!(json["version_name"], "32.17.0.100");
        assert_eq!(json["version_code"], 42);
        assert_eq!(json["total_size"], 15);
        assert_eq!(json["date_created"], "2026-09-22T06:50:00");
        assert_eq!(json["file_paths"].as_array().unwrap().len(), 2);
        assert_eq!(json["file_paths"][0], "base.apk");
        let app = &json["app_list"][0];
        assert_eq!(app["file_paths"][1], "split_config.arm64_v8a.apk");
        assert_eq!(app["name"], "亚马逊购物");
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
            reserve_file_name(dir.path(), "亚马逊购物 1.0", "apk")
                .await
                .unwrap(),
            "亚马逊购物 1.0.apk"
        );
        tokio::fs::write(dir.path().join("亚马逊购物 1.0.apk"), b"x")
            .await
            .unwrap();
        assert_eq!(
            reserve_file_name(dir.path(), "亚马逊购物 1.0", "apk")
                .await
                .unwrap(),
            "亚马逊购物 1.0 (2).apk"
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
        assert_eq!(artifact.file_name, "亚马逊购物 32.17.xapk");
        assert!(artifact.path.starts_with(&out));
        assert!(artifact.bytes > base.len() as u64 + split.len() as u64);
        let entries = entries_of(&artifact.path).await;
        assert_eq!(
            entries
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["manifest.json", "base.apk", "split_config.zh.apk"]
        );
        assert_eq!(entries[1].1, base, "APK 字节必须原样保留");
        assert_eq!(entries[2].1, split);
        let manifest: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&entries[0].1).unwrap();
        assert_eq!(
            manifest["total_size"],
            base.len() as u64 + split.len() as u64
        );
        // 容器名不许改写：安装器按 base.apk / split_*.apk 识别分包
        assert_eq!(manifest["file_paths"][1], "split_config.zh.apk");
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
        assert_eq!(artifact.file_name, "剪贴岛 1.2.3.apk");
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
