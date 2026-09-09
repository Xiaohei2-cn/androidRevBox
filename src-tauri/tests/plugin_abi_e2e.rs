//! P4 集成回测：真实 cdylib 全链路 load→info→call→free→shutdown。
//! 依赖 dev-dependency 的 plugin-crypto-base64 保证测试运行前 cdylib 已构建。
//! 产物定位用 current_exe() 反推 target 目录（比 env 拼路径更稳，不受 custom target-dir 影响）。

use std::path::{Path, PathBuf};

use app_reverse_tools_lib::plugins::loader;

/// 由测试可执行文件路径反推 cargo target 目录：
/// <target>/debug(deps)/app_reverse_tools_lib-xxxx → 上溯到 <target>/<profile>
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let deps = exe.parent().expect("deps dir");
    let profile = deps.parent().expect("profile dir");
    profile.to_path_buf()
}

/// 确保插件 cdylib 已构建：dev-dependency 只编 rlib，需显式 build 出 .dylib/.dll/.so。
/// （测试运行期外层 cargo 已完成构建、不持 target 锁，嵌套 cargo 安全——escargot 同款做法）
/// 每个测试进程强制重建一次：「产物存在」≠「产物新鲜」，旧 dylib 会导致协议假失败。
fn ensure_cdylib() -> PathBuf {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let ws = profile_dir()
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let status = std::process::Command::new(&cargo)
            .args(["build", "-p", "plugin-crypto-base64", "--lib"])
            .current_dir(&ws)
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "构建插件 cdylib 失败");
    });
    let path = cdylib_path();
    assert!(
        path.exists(),
        "构建插件 cdylib 失败，期望产物 {}",
        path.display()
    );
    path
}

fn cdylib_path() -> PathBuf {
    let (prefix, suffix) = if cfg!(windows) {
        ("", ".dll")
    } else if cfg!(target_os = "macos") {
        ("lib", ".dylib")
    } else {
        ("lib", ".so")
    };
    profile_dir().join(format!("{prefix}crypto_base64{suffix}"))
}

/// 组装一个最小受控插件目录（manifest + 真实产物），返回其路径
fn stage_plugin_dir(dir: &Path, manifest_text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest_text).unwrap();
    // 拷贝真实 cdylib 到 manifest.entry 指定的相对子目录
    let plat = platform_key();
    let sub = dir.join(&plat);
    std::fs::create_dir_all(&sub).unwrap();
    let artifact_name = cdylib_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let lib = ensure_cdylib();
    std::fs::copy(&lib, sub.join(&artifact_name))
        .unwrap_or_else(|e| panic!("copy cdylib {}: {e}", lib.display()));
}

fn platform_key() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

fn manifest_json(abi: u32, entry_key: &str, entry_path: &str) -> String {
    format!(
        r#"{{
          "id": "crypto.base64",
          "name": "Base64",
          "version": "1.0.0",
          "abi": {abi},
          "type": "crypto",
          "entry": {{ "{entry_key}": "{entry_path}" }},
          "capabilities": ["encode", "decode"]
        }}"#
    )
}

#[test]
fn full_lifecycle_load_info_call_free_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let plat = platform_key();
    let artifact = cdylib_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let root = tmp.path();
    stage_plugin_dir(
        root.join("crypto-base64").as_path(),
        &manifest_json(1, &plat, &format!("{plat}/{artifact}")),
    );

    let (loaded, errors) = loader::discover_and_load(root);
    assert!(errors.is_empty(), "加载不应有错误: {errors:?}");
    assert_eq!(loaded.len(), 1, "应加载一个插件");

    let plugin = loaded.get("crypto.base64").expect("crypto.base64 已加载");

    // ABI 自报信息与 manifest 一致
    let abi = plugin.abi_info();
    assert_eq!(abi.abi_version, 1);
    assert_eq!(abi.id, "crypto.base64");
    assert_eq!(abi.plugin_type, "crypto");

    // call encode（JSON 协议）
    let (code, out) = plugin.call(r#"{"op":"encode","data":"hello 世界"}"#.as_bytes());
    assert_eq!(code, 0, "encode 成功");
    let resp: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(resp["ok"], true);
    let b64 = resp["data"].as_str().unwrap().to_string();
    assert!(!b64.is_empty());

    // call decode 回环（输出内存已被 host 复制并归还，二次调用验证 free 无泄漏/悬垂）
    let req = format!(r#"{{"op":"decode","data":"{b64}"}}"#);
    let (code2, out2) = plugin.call(req.as_bytes());
    assert_eq!(code2, 0);
    let resp2: serde_json::Value = serde_json::from_slice(&out2).unwrap();
    assert_eq!(resp2["data"], "hello 世界");

    // shutdown：drop 触发（LoadedPlugin::drop → at_plugin_shutdown）
    assert_eq!(plugin.manifest().capabilities, vec!["encode", "decode"]);
    // 借用随作用域结束；显式 drop map → Arc 归零 → LoadedPlugin::drop → at_plugin_shutdown
    drop(loaded);
}

#[test]
fn rejects_abi_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let plat = platform_key();
    let artifact = cdylib_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    stage_plugin_dir(
        tmp.path().join("p").as_path(),
        &manifest_json(999, &plat, &format!("{plat}/{artifact}")),
    );
    let (loaded, errors) = loader::discover_and_load(tmp.path());
    assert!(loaded.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].1.to_string().contains("abi") || errors[0].1.to_string().contains("ABI"),
        "错误应说明 ABI 不兼容: {}",
        errors[0].1
    );
}

#[test]
fn rejects_missing_platform_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let artifact = cdylib_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    stage_plugin_dir(
        tmp.path().join("p").as_path(),
        &manifest_json(1, "some-other-platform", &format!("x/{artifact}")),
    );
    let (loaded, errors) = loader::discover_and_load(tmp.path());
    assert!(loaded.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].1.to_string().contains("平台产物"),
        "应提示缺当前平台产物: {}",
        errors[0].1
    );
}

#[test]
fn rejects_bad_manifest_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), "{ not json").unwrap();
    let (loaded, errors) = loader::discover_and_load(tmp.path());
    assert!(loaded.is_empty());
    assert_eq!(errors.len(), 1);
    let msg = errors[0].1.to_string();
    assert!(msg.contains("manifest"), "{msg}");
}

#[test]
fn rejects_id_mismatch_between_manifest_and_library() {
    // manifest 声明的 id 与动态库 at_plugin_info 自报 id 不一致 → 拒绝（防目录错位）
    let tmp = tempfile::tempdir().unwrap();
    let plat = platform_key();
    let artifact = cdylib_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let text = manifest_json(1, &plat, &format!("{plat}/{artifact}"))
        .replace("crypto.base64", "crypto.wrongid");
    stage_plugin_dir(tmp.path().join("p").as_path(), &text);
    let (loaded, errors) = loader::discover_and_load(tmp.path());
    assert!(loaded.is_empty());
    assert_eq!(errors.len(), 1);
    let msg = errors[0].1.to_string();
    assert!(msg.contains("实际") || msg.contains("id"), "{msg}");
}
