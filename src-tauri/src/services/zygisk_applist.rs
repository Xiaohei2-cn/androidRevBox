//! 独立 Zygisk Applist 模块的线协议适配器。
//! 模块本身是可独立安装的 C++/Java 工程，桌面端只依赖稳定 wire protocol。

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::adapters::adb;
use crate::core::error::{CoreError, CoreResult};
use crate::services::device_service::{AdbRunOutput, AdbRunner};

const MODULE_PORT: u16 = 11_500;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskAppItem {
    /// 模块线协议字段名是 `pkg`；对前端保持 `packageName`（camelCase）不变。
    #[serde(alias = "pkg")]
    pub package_name: String,
    pub label: String,
    pub version_name: String,
    pub version_code: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskApkFile {
    pub package_name: String,
    pub name: String,
    pub size: u64,
}

/// E 清单中的单个 APK 文件（设备侧路径仅用于展示与校验，不参与本地落盘路径拼接）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskApkManifestEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZygiskExportReport {
    pub package_name: String,
    pub files: Vec<ZygiskApkFile>,
    pub destination: String,
    pub bytes: u64,
}

pub struct ZygiskApplistService {
    runner: Arc<dyn AdbRunner>,
}

impl ZygiskApplistService {
    pub fn new(runner: Arc<dyn AdbRunner>) -> Self {
        Self { runner }
    }

    /// Q：由 Framework PackageManager 解析设备当前 locale 的应用显示名。
    pub async fn list(&self, serial: &str) -> CoreResult<Vec<ZygiskAppItem>> {
        self.with_module(serial, |mut stream| async move {
            stream.write_all(b"Q").await.map_err(io_error)?;
            let line = read_line(&mut stream, MAX_JSON_BYTES).await?;
            let value: serde_json::Value = serde_json::from_slice(&line).map_err(|error| {
                CoreError::Internal(format!("Zygisk 应用清单 JSON 无法解析: {error}"))
            })?;
            if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
                return Err(CoreError::Internal(format!("Zygisk 应用清单失败: {error}")));
            }
            serde_json::from_value(value)
                .map_err(|error| CoreError::Internal(format!("Zygisk 应用清单字段不兼容: {error}")))
        })
        .await
    }

    /// E：每包 base + split APK 的文件清单（名称、设备路径、字节数），不含文件体。
    pub async fn apk_manifest(
        &self,
        serial: &str,
    ) -> CoreResult<BTreeMap<String, Vec<ZygiskApkManifestEntry>>> {
        self.with_module(serial, |mut stream| async move {
            stream.write_all(b"E").await.map_err(io_error)?;
            let line = read_line(&mut stream, MAX_JSON_BYTES).await?;
            parse_apk_manifest(&line)
        })
        .await
    }

    /// D：流式导出一个包的 base.apk 与全部 split APK。
    pub async fn export_package(
        &self,
        serial: &str,
        package_name: &str,
        destination: &Path,
    ) -> CoreResult<ZygiskExportReport> {
        validate_package_name(package_name)?;
        let destination = destination.to_path_buf();
        let package_name = package_name.to_string();
        self.with_module(serial, move |mut stream| async move {
            stream.write_all(b"D").await.map_err(io_error)?;
            stream
                .write_all(package_name.as_bytes())
                .await
                .map_err(io_error)?;
            stream.write_all(b"\n\n").await.map_err(io_error)?;

            let package_dir = destination.join(&package_name);
            tokio::fs::create_dir_all(&package_dir)
                .await
                .map_err(io_error)?;
            let mut files = Vec::new();
            let mut total_bytes = 0_u64;
            loop {
                let line = read_line(&mut stream, 32 * 1024).await?;
                if line == b"DONE" {
                    break;
                }
                if line.starts_with(b"ERR") {
                    return Err(CoreError::Internal(format!(
                        "Zygisk APK 导出失败: {}",
                        String::from_utf8_lossy(&line)
                    )));
                }
                let header = parse_file_header(&line)?;
                if header.package_name != package_name {
                    return Err(CoreError::Internal(format!(
                        "Zygisk 返回了意外包名 {}",
                        header.package_name
                    )));
                }
                if header.size > MAX_APK_BYTES {
                    return Err(CoreError::Internal("APK 文件超过 512 MiB 限制".into()));
                }
                let safe_name = safe_filename(&header.name)?;
                let target = package_dir.join(&safe_name);
                let temporary = package_dir.join(format!(".{}.part", safe_name));
                let mut file = tokio::fs::File::create(&temporary)
                    .await
                    .map_err(io_error)?;
                copy_exact(&mut stream, &mut file, header.size).await?;
                file.flush().await.map_err(io_error)?;
                drop(file);
                tokio::fs::rename(&temporary, &target)
                    .await
                    .map_err(io_error)?;
                total_bytes = total_bytes.saturating_add(header.size);
                files.push(ZygiskApkFile {
                    package_name: header.package_name,
                    name: safe_name,
                    size: header.size,
                });
            }
            Ok(ZygiskExportReport {
                package_name,
                files,
                destination: package_dir.to_string_lossy().into_owned(),
                bytes: total_bytes,
            })
        })
        .await
    }

    async fn with_module<F, Fut, T>(&self, serial: &str, operation: F) -> CoreResult<T>
    where
        F: FnOnce(TcpStream) -> Fut,
        Fut: std::future::Future<Output = CoreResult<T>>,
    {
        if serial.trim().is_empty() {
            return Err(CoreError::Internal("设备 serial 不能为空".into()));
        }
        let environment = self.runner.environment().await;
        let adb_path = environment
            .path
            .ok_or_else(|| CoreError::Internal("adb 不可用，无法连接 Zygisk 模块".into()))?;
        let forward = self
            .runner
            .run(
                &adb_path,
                &adb::build_args(
                    Some(serial),
                    &adb::cmd_forward("tcp:0", &format!("tcp:{}", MODULE_PORT)),
                ),
                FORWARD_TIMEOUT,
            )
            .await?;
        ensure_adb_success("建立 Zygisk forward", &forward)?;
        let port = adb::parse_dynamic_forward_port(&forward.stdout).ok_or_else(|| {
            CoreError::Internal(format!(
                "adb forward 未返回动态端口: {}",
                forward.stdout.trim()
            ))
        })?;
        let local = format!("tcp:{}", port);
        let result = match timeout(REQUEST_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await {
            Ok(Ok(stream)) => timeout(REQUEST_TIMEOUT, operation(stream))
                .await
                .map_err(|_| CoreError::Internal("Zygisk 请求超时".into()))
                .and_then(|result| result),
            Ok(Err(error)) => Err(CoreError::Internal(format!(
                "无法连接 Zygisk 模块: {error}"
            ))),
            Err(_) => Err(CoreError::Internal("连接 Zygisk 模块超时".into())),
        };
        let cleanup = self
            .runner
            .run(
                &adb_path,
                &adb::build_args(Some(serial), &adb::cmd_forward_remove(Some(&local))),
                FORWARD_TIMEOUT,
            )
            .await;
        let cleanup = cleanup.and_then(|output| {
            ensure_adb_success("清理 Zygisk forward", &output)?;
            Ok(())
        });
        if result.is_ok()
            && let Err(error) = cleanup
        {
            return Err(error);
        }
        result
    }
}

#[derive(Debug)]
struct FileHeader {
    size: u64,
    package_name: String,
    name: String,
}

fn parse_apk_manifest(line: &[u8]) -> CoreResult<BTreeMap<String, Vec<ZygiskApkManifestEntry>>> {
    let value: serde_json::Value = serde_json::from_slice(line)
        .map_err(|error| CoreError::Internal(format!("Zygisk APK 清单 JSON 无法解析: {error}")))?;
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(CoreError::Internal(format!("Zygisk APK 清单失败: {error}")));
    }
    serde_json::from_value(value)
        .map_err(|error| CoreError::Internal(format!("Zygisk APK 清单字段不兼容: {error}")))
}

fn parse_file_header(line: &[u8]) -> CoreResult<FileHeader> {
    let text = std::str::from_utf8(line)
        .map_err(|_| CoreError::Internal("Zygisk APK 文件头不是 UTF-8".into()))?;
    let mut parts = text.splitn(4, ' ');
    if parts.next() != Some("F") {
        return Err(CoreError::Internal(format!("Zygisk 未知文件头: {text}")));
    }
    let size = parts
        .next()
        .ok_or_else(|| CoreError::Internal("Zygisk 文件头缺少大小".into()))?
        .parse::<u64>()
        .map_err(|_| CoreError::Internal("Zygisk 文件大小非法".into()))?;
    let package_name = parts
        .next()
        .ok_or_else(|| CoreError::Internal("Zygisk 文件头缺少包名".into()))?;
    let name = parts
        .next()
        .ok_or_else(|| CoreError::Internal("Zygisk 文件头缺少文件名".into()))?;
    validate_package_name(package_name)?;
    Ok(FileHeader {
        size,
        package_name: package_name.to_string(),
        name: name.to_string(),
    })
}

fn validate_package_name(package_name: &str) -> CoreResult<()> {
    if package_name.is_empty()
        || package_name.len() > 255
        || package_name.contains('/')
        || package_name.contains('\\')
        || package_name.contains("..")
    {
        return Err(CoreError::Internal("包名非法".into()));
    }
    Ok(())
}

fn safe_filename(name: &str) -> CoreResult<String> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CoreError::Internal(
            "Zygisk 返回了不安全的 APK 文件名".into(),
        ));
    }
    Ok(name.to_string())
}

async fn read_line(stream: &mut TcpStream, max: usize) -> CoreResult<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.map_err(io_error)?;
        if byte[0] == b'\n' {
            return Ok(line);
        }
        line.push(byte[0]);
        if line.len() > max {
            return Err(CoreError::Internal("Zygisk 响应行超过大小限制".into()));
        }
    }
}

async fn copy_exact(
    stream: &mut TcpStream,
    file: &mut tokio::fs::File,
    size: u64,
) -> CoreResult<()> {
    let mut remaining = size;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        stream
            .read_exact(&mut buffer[..want])
            .await
            .map_err(io_error)?;
        file.write_all(&buffer[..want]).await.map_err(io_error)?;
        remaining -= want as u64;
    }
    Ok(())
}

fn io_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Internal(format!("Zygisk I/O 失败: {error}"))
}

fn ensure_adb_success(action: &str, output: &AdbRunOutput) -> CoreResult<()> {
    if output.exit_code == Some(0) {
        Ok(())
    } else {
        Err(CoreError::Internal(format!(
            "{action}失败: {}",
            output.stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_export_header_with_spaces_in_filename() {
        let header = parse_file_header(b"F 123 com.example base split.apk").unwrap();
        assert_eq!(header.size, 123);
        assert_eq!(header.package_name, "com.example");
        assert_eq!(header.name, "base split.apk");
    }

    #[test]
    fn rejects_path_traversal_from_module() {
        assert!(safe_filename("../evil.apk").is_err());
        assert!(safe_filename("/tmp/evil.apk").is_err());
        assert!(validate_package_name("com.example/app").is_err());
    }

    #[test]
    fn accepts_normal_apk_names_and_package_names() {
        assert_eq!(
            safe_filename("split_config.zh.apk").unwrap(),
            "split_config.zh.apk"
        );
        assert!(validate_package_name("com.example.app").is_ok());
    }

    #[test]
    fn maps_module_wire_fields_to_camel_case_dto() {
        // 真机 applist 模块的真实报文形状：pkg / label / versionName / versionCode
        let line = br#"[{"pkg":"com.czb.chezhubang","label":"\u56e2\u6cb9","versionName":"7.6.4","versionCode":204}]"#;
        let value: serde_json::Value = serde_json::from_slice(line).unwrap();
        let apps: Vec<ZygiskAppItem> = serde_json::from_value(value).unwrap();
        assert_eq!(apps[0].package_name, "com.czb.chezhubang");
        assert_eq!(apps[0].label, "团油");
        assert_eq!(apps[0].version_name, "7.6.4");
        assert_eq!(apps[0].version_code, 204);
        let out = serde_json::to_value(&apps).unwrap();
        assert_eq!(out[0]["packageName"], "com.czb.chezhubang");
        assert_eq!(out[0]["versionCode"], 204);
    }

    #[test]
    fn parses_manifest_map_grouped_by_package() {
        let line = br#"{"com.example":[{"name":"base.apk","path":"/app/base.apk","size":123},{"name":"split_config.zh.apk","path":"/app/split_zh.apk","size":45}]}"#;
        let manifest = parse_apk_manifest(line).unwrap();
        let files = manifest.get("com.example").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "base.apk");
        assert_eq!(files[0].size, 123);
        assert_eq!(files[1].path, "/app/split_zh.apk");
    }

    #[test]
    fn surfaces_module_error_from_manifest() {
        let error = parse_apk_manifest(br#"{"error":"helper crashed"}"#).unwrap_err();
        assert!(error.to_string().contains("helper crashed"));
    }

    #[tokio::test]
    #[ignore = "需要已安装 applist 模块并重启生效的真机；APPLIST_TEST_SERIAL=<serial> cargo test real_zygisk_module -- --ignored --nocapture"]
    async fn real_zygisk_module_query_manifest_and_export() {
        use crate::db::Db;
        use crate::services::config_service::ConfigService;
        use crate::services::device_service::RealAdbRunner;
        use std::collections::HashSet;

        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config));
        let service = ZygiskApplistService::new(runner.clone());

        let apps = service.list(&serial).await.expect("Q 查询失败");
        assert!(apps.len() >= 5, "Zygisk 应用清单过少: {}", apps.len());
        assert!(
            apps.iter()
                .all(|app| !app.package_name.trim().is_empty() && !app.label.trim().is_empty()),
            "存在空包名或空显示名"
        );
        assert!(
            apps.iter().any(|app| app.label != app.package_name),
            "全部 label 都等于包名，疑似未经 Framework 本地化解析"
        );

        let environment = runner.environment().await;
        let adb_path = environment.path.expect("adb 不可用");
        let legacy = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_list_packages(true)),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        let legacy_pkgs = adb::parse_packages(&legacy.stdout);
        let zygisk_pkgs: HashSet<&str> = apps.iter().map(|app| app.package_name.as_str()).collect();
        let missing: Vec<&str> = legacy_pkgs
            .iter()
            .filter(|pkg| !zygisk_pkgs.contains(pkg.as_str()))
            .map(String::as_str)
            .collect();
        assert!(missing.is_empty(), "Zygisk 清单缺少 pm 三方包: {missing:?}");

        let manifest = service.apk_manifest(&serial).await.expect("E 清单失败");
        assert!(!manifest.is_empty(), "APK 清单为空");
        let (target, entries) = manifest
            .iter()
            .filter(|(pkg, files)| !files.is_empty() && zygisk_pkgs.contains(pkg.as_str()))
            .min_by_key(|(_, files)| files.iter().map(|file| file.size).sum::<u64>())
            .expect("没有可导出的包");

        let out_dir = tempfile::tempdir().unwrap();
        let report = service
            .export_package(&serial, target, out_dir.path())
            .await
            .expect("D 导出失败");
        assert_eq!(report.package_name, *target);
        let mut expected: Vec<(String, u64)> = entries
            .iter()
            .map(|file| (file.name.clone(), file.size))
            .collect();
        let mut actual: Vec<(String, u64)> = report
            .files
            .iter()
            .map(|file| (file.name.clone(), file.size))
            .collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected, "导出的 APK 集合与 E 清单不一致");
        assert_eq!(
            report.bytes,
            expected.iter().map(|(_, size)| *size).sum::<u64>()
        );
        for (name, size) in &expected {
            let bytes = std::fs::read(out_dir.path().join(target).join(name)).unwrap();
            assert_eq!(bytes.len() as u64, *size, "{target}/{name} 大小不符");
            assert!(
                bytes.starts_with(b"PK"),
                "{target}/{name} 缺少 zip 魔数，导出流可能被截断"
            );
        }

        let forwards = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_forward_list()),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert!(
            !forwards.stdout.contains(&format!(":{MODULE_PORT}")),
            "Zygisk forward 未清理: {}",
            forwards.stdout
        );
    }
}
