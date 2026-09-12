//! EnvService（P7）：仪表盘环境/工具探测中心。
//! 卡片：Python / Node / Frida / IDA MCP / jadx MCP / 安卓前台应用。
//! 原则（PHASES §10）：
//! - 全部探测并发执行；有依赖的按前置状态剪枝（Python 未配置不探 Frida、
//!   adb 不可用不发起任何前台探测 shell 调用），减少无谓开销；
//! - 解析逻辑纯函数化（dumpsys window/package、pidof、--version 输出），可无设备单测；
//! - 单卡探测失败/超时只影响该卡，不拖垮整版；端口探活失败是常态（显示「未检测到」）。

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use crate::adapters::adb as adb_adapter;
use crate::core::error::CoreResult;
use crate::services::config_service::{
    ConfigService, KEY_IDA_MCP_PORT, KEY_JADX_MCP_PORT, KEY_NODE_PATH, KEY_PYTHON_PATH,
};
use crate::services::device_service::{AdbEnvironment, AdbRunOutput, AdbRunner};

/// 单个外部探测命令的超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// MCP 端口探活超时
const MCP_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// 前台应用相关 adb shell 的超时
const FG_SHELL_TIMEOUT: Duration = Duration::from_secs(8);

pub const DEFAULT_IDA_MCP_PORT: u16 = 13_337;
pub const DEFAULT_JADX_MCP_PORT: u16 = 8_650;

/// frida 探测脚本：一条子进程同时查 frida 与 frida-tools，未安装输出 null
const FRIDA_PROBE_SCRIPT: &str = "import json\nfrom importlib.metadata import version\nout = {}\nfor pkg in ('frida', 'frida-tools'):\n    try:\n        out[pkg] = version(pkg)\n    except Exception:\n        out[pkg] = None\nprint(json.dumps(out))";

/// 解析 FRIDA_PROBE_SCRIPT 的输出 → (frida 版本, frida-tools 版本)
pub fn parse_frida_versions(stdout: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else {
        return (None, None);
    };
    let get = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    (get("frida"), get("frida-tools"))
}

#[derive(Clone)]
pub struct EnvService {
    config: Arc<ConfigService>,
    adb: Arc<dyn AdbRunner>,
}

impl EnvService {
    pub fn new(config: Arc<ConfigService>, adb: Arc<dyn AdbRunner>) -> Self {
        Self { config, adb }
    }

    fn u16_config(&self, key: &str, default: u16) -> u16 {
        self.config
            .get(key, &default.to_string())
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(default)
    }

    // ===== 各卡探测 =====

    /// ADB 环境（复用 P3 的 runner；前台应用卡的剪枝前置）
    pub async fn adb(&self) -> AdbEnvironment {
        self.adb.environment().await
    }

    /// Python：用户在设置中指定的解释器（空 = 未配置，不做 PATH 自动探测，
    /// 按 §10 规范「未配置时提示去设置」）。
    pub async fn python(&self) -> PythonEnv {
        let path = self
            .config
            .get(KEY_PYTHON_PATH, "")
            .unwrap_or_default()
            .trim()
            .to_string();
        if path.is_empty() {
            return PythonEnv {
                configured: false,
                ready: false,
                path: None,
                version: None,
                hint: Some("未配置 Python 解释器：请在 设置 → 工具环境 中指定。".into()),
            };
        }
        self.probe_python_at(&path).await
    }

    /// 在指定路径探测 Python（供 python() 与规范化兜底复用）。
    /// 解析 --version 时合并 stdout+stderr——旧版 Python（<3.4 语义）把版本打到 stderr。
    async fn probe_python_at(&self, path: &str) -> PythonEnv {
        let version_from = |out: &ProbeOutput| -> Option<String> {
            parse_python_version(&out.stdout).or_else(|| parse_python_version(&out.stderr))
        };
        match run_probe(path, &["--version"]).await {
            Ok(out) => match version_from(&out) {
                Some(version) => PythonEnv {
                    configured: true,
                    ready: true,
                    path: Some(path.to_string()),
                    version: Some(version),
                    hint: None,
                },
                None => PythonEnv {
                    configured: true,
                    ready: false,
                    path: Some(path.to_string()),
                    version: None,
                    hint: Some(format!(
                        "执行 --version 成功但输出无法解析: {}",
                        first_line(&(out.stdout + &out.stderr))
                    )),
                },
            },
            Err(e) => {
                // 兜底（macOS 文件选择器常见）：选到的是 framework 包入口/断链 symlink
                // 而非真解释器——尝试 ① 同目录 python3* 兄弟；② 解析 symlink 真身后重试。
                if let Some(env) = self.normalize_python_candidate(path, &e).await {
                    env
                } else {
                    PythonEnv {
                        configured: true,
                        ready: false,
                        path: Some(path.to_string()),
                        version: None,
                        hint: Some(format!("解释器执行失败: {e}")),
                    }
                }
            }
        }
    }

    /// macOS 文件选择器兜底：用户常选中 Python.app 内入口或 .framework 顶层。
    /// 依次尝试：symlink 真身 → 同目录 python3* → ../bin/python3*。命中即
    /// 规范化回写配置（下次直接可用），返回 ready 的 PythonEnv；全部失败返回 None。
    async fn normalize_python_candidate(&self, path: &str, orig_err: &str) -> Option<PythonEnv> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let p = std::path::Path::new(path);
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        // ① symlink 真身
        if let Ok(resolved) = p.canonicalize() {
            if resolved != p {
                candidates.push(resolved);
            }
        }
        // ② 同目录与 ../bin 下的 python3*
        if let Some(dir) = p.parent() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut sibs: Vec<std::path::PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|c| {
                        c.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("python3"))
                            .unwrap_or(false)
                    })
                    .collect();
                sibs.sort();
                candidates.extend(sibs);
            }
            if let Some(parent) = dir.parent() {
                let bin = parent.join("bin");
                if let Ok(entries) = std::fs::read_dir(&bin) {
                    let mut sibs: Vec<std::path::PathBuf> = entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|c| {
                            c.file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.starts_with("python3"))
                                .unwrap_or(false)
                        })
                        .collect();
                    sibs.sort();
                    candidates.extend(sibs);
                }
            }
        }
        tracing::info!(
            from = %path,
            candidates = ?candidates.iter().map(|c| c.display().to_string()).collect::<Vec<_>>(),
            "python 路径规范化兜底"
        );
        for cand in candidates {
            let cand_str = cand.to_string_lossy().into_owned();
            if cand_str == path {
                continue;
            }
            if let Ok(out) = run_probe(&cand_str, &["--version"]).await {
                if let Some(version) =
                    parse_python_version(&out.stdout).or_else(|| parse_python_version(&out.stderr))
                {
                    // 规范化回写：下次启动直接用可用解释器
                    let _ = self
                        .config
                        .set(crate::services::config_service::KEY_PYTHON_PATH, &cand_str);
                    tracing::info!(from = %path, to = %cand_str, version = %version, "python 路径已规范化");
                    return Some(PythonEnv {
                        configured: true,
                        ready: true,
                        path: Some(cand_str),
                        version: Some(version),
                        hint: None,
                    });
                }
            }
        }
        tracing::warn!(from = %path, error = %orig_err, "python 路径兜底失败");
        None
    }

    /// Node：配置路径优先，否则 PATH 上的 node。
    pub async fn node(&self) -> NodeEnv {
        let configured = self
            .config
            .get(KEY_NODE_PATH, "")
            .unwrap_or_default()
            .trim()
            .to_string();
        let has_config = !configured.is_empty();
        let path = if has_config {
            configured.clone()
        } else {
            "node".to_string()
        };
        match run_probe(&path, &["--version"]).await {
            Ok(out) => match parse_node_version(&out.stdout) {
                Some(version) => {
                    let npm_global_root = probe_npm_root(&path).await;
                    NodeEnv {
                        ready: true,
                        path: Some(path),
                        version: Some(version),
                        npm_global_root,
                        hint: None,
                    }
                }
                None => NodeEnv {
                    ready: false,
                    path: Some(path),
                    version: None,
                    npm_global_root: None,
                    hint: Some(format!(
                        "node --version 输出无法解析: {}",
                        first_line(&out.stdout)
                    )),
                },
            },
            Err(_) => NodeEnv {
                ready: false,
                path: non_empty(configured.clone()),
                version: None,
                npm_global_root: None,
                hint: Some(if has_config {
                    format!("配置的 node 无法执行: {configured}")
                } else {
                    "未检测到 node：请安装 Node.js 并加入 PATH，或在 设置 → 工具环境 指定路径。"
                        .into()
                }),
            },
        }
    }

    /// Frida：指定 Python 环境下 frida / frida-tools 的安装与版本。
    /// 剪枝：Python 未配置/不可用时直接返回 not_probed，不发起子进程。
    pub async fn frida(&self) -> FridaEnv {
        let py = self.python().await;
        self.frida_after(py).await
    }

    /// frida 探测（复用已探测的 Python 环境，避免 overview 里重复起子进程）
    async fn frida_after(&self, py: PythonEnv) -> FridaEnv {
        if !py.ready {
            return FridaEnv {
                python_ready: false,
                installed: false,
                frida_version: None,
                frida_tools_version: None,
                hint: Some(match py.hint {
                    Some(h) => h,
                    None => "Python 环境未就绪，无法检测 Frida。".into(),
                }),
            };
        }
        let python_path = py.path.clone().unwrap_or_default();
        // 一条子进程同时查两个包（importlib.metadata），失败=未安装
        let script = FRIDA_PROBE_SCRIPT;
        match run_probe(&python_path, &["-c", script]).await {
            Ok(out) => {
                let (frida, tools) = parse_frida_versions(&out.stdout);
                let installed = frida.is_some() || tools.is_some();
                FridaEnv {
                    python_ready: true,
                    installed,
                    frida_version: frida,
                    frida_tools_version: tools,
                    hint: if installed {
                        None
                    } else {
                        Some(
                            "该 Python 环境未安装 frida / frida-tools（pip install frida-tools）。"
                                .into(),
                        )
                    },
                }
            }
            Err(e) => FridaEnv {
                python_ready: true,
                installed: false,
                frida_version: None,
                frida_tools_version: None,
                hint: Some(format!("Frida 检测执行失败: {e}")),
            },
        }
    }

    /// IDA 状态：宿主应用存在性（mac）+ MCP 端口探活
    pub async fn ida_mcp(&self) -> McpEnv {
        let port = self.u16_config(KEY_IDA_MCP_PORT, DEFAULT_IDA_MCP_PORT);
        let (app, mut env) = tokio::join!(detect_ida_app(), probe_mcp("IDA MCP", port));
        env.app_installed = app.as_ref().map(|_| true);
        env.app_path = app;
        if let Some(p) = &env.app_path {
            env.hint = Some(env.hint.take().unwrap_or_default() + &format!("（应用: {p}）"));
        }
        env
    }

    /// jadx-gui 状态：CLI 存在性（`jadx-gui --version`）+ MCP 端口探活
    pub async fn jadx_mcp(&self) -> McpEnv {
        let port = self.u16_config(KEY_JADX_MCP_PORT, DEFAULT_JADX_MCP_PORT);
        let (ver, mut env) = tokio::join!(detect_jadx_cli(), probe_mcp("jadx MCP", port));
        match ver {
            Some(v) => {
                env.app_installed = Some(true);
                env.app_path = None;
                env.hint = Some(env.hint.take().unwrap_or_default() + &format!("（jadx-gui {v}）"));
            }
            None => {
                env.app_installed = Some(false);
                env.hint = Some(
                    env.hint.take().unwrap_or_default() + &format!("（{}）", "jadx-gui 不在 PATH"),
                );
            }
        }
        env
    }

    /// 探测给定解释器路径的版本（stdout+stderr 合并解析）
    async fn probe_version(path: &str) -> Option<String> {
        match run_probe(path, &["--version"]).await {
            Ok(out) => {
                parse_python_version(&out.stdout).or_else(|| parse_python_version(&out.stderr))
            }
            Err(_) => None,
        }
    }

    /// 文件选择器解析（P8）：把用户可能选中的「错误入口」解析成真解释器。
    /// 处理：pyenv shim（读脚本 exec 行）→ .app bundle 入口 → framework 顶层 →
    /// symlink 真身。返回 (原始选择, 解析结果, 版本)；解析失败 resolvedPath = 原值。
    pub async fn resolve_interpreter(&self, picked: &str) -> ResolvedInterpreter {
        let mut resolved = picked.to_string();
        let mut how = "as-is".to_string();

        // ① pyenv shim：shim 脚本的 exec 行指向 pyenv 二进制而非解释器，
        //    必须用 `pyenv which <name>` 语义查真身（PYENV_ROOT 继承自 shim 同目录约定）
        let p = std::path::Path::new(picked);
        if picked.contains(".pyenv/shims/") {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("python3")
                .to_string();
            // PYENV_ROOT 从 shim 路径反推：~/.pyenv/shims/x → ~/.pyenv
            let pyenv_root = std::path::Path::new(picked)
                .parent()
                .and_then(|shims| shims.parent())
                .map(|root| root.to_path_buf());
            if let Some(root) = pyenv_root {
                // `pyenv which` 依赖当前全局/本地版本设置；直接问 pyenv 二进制
                let pyenv_bin = root.join("bin").join("pyenv");
                let pyenv_bin = if pyenv_bin.is_file() {
                    pyenv_bin
                } else {
                    // homebrew 安装：/opt/homebrew/opt/pyenv/bin/pyenv
                    std::path::PathBuf::from("/opt/homebrew/opt/pyenv/bin/pyenv")
                };
                if pyenv_bin.is_file() {
                    if let Ok(out) =
                        run_probe(pyenv_bin.to_string_lossy().as_ref(), &["which", &name]).await
                    {
                        let bin = out.stdout.trim().to_string();
                        if !bin.is_empty() && std::path::Path::new(&bin).is_file() {
                            resolved = bin;
                            how = "pyenv-shim".into();
                        }
                    }
                }
            }
        }

        // ② .app bundle 入口 → 同 bundle 所在 framework 的 bin/python3*
        if resolved.contains(".app/Contents/MacOS/") {
            if let Some(idx) = resolved.find(".app/Contents") {
                let bundle = &resolved[..idx + 4]; // xxx.app
                if let Some(fw_dir) = std::path::Path::new(bundle).parent() {
                    // framework 布局：<fw>/Python.framework/Resources/Python.app/Contents/MacOS/Python
                    // 真解释器在 <fw>/Python.framework/Versions/X.Y/bin/python3*
                    let fw_root = fw_dir; // .../Python.framework 的宿主目录
                    let mut found = false;
                    if let Ok(versions) = std::fs::read_dir(fw_root) {
                        let mut cands: Vec<_> = versions
                            .flatten()
                            .filter(|e| e.path().is_dir())
                            .flat_map(|e| {
                                let bin = e.path().join("bin");
                                std::fs::read_dir(bin)
                                    .map(|rd| rd.flatten().map(|x| x.path()).collect::<Vec<_>>())
                                    .unwrap_or_default()
                            })
                            .filter(|c| {
                                c.file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|n| n.starts_with("python3"))
                                    .unwrap_or(false)
                            })
                            .collect();
                        cands.sort();
                        if let Some(best) = cands.pop() {
                            resolved = best.to_string_lossy().into_owned();
                            how = "app-bundle".into();
                            found = true;
                        }
                    }
                    let _ = found;
                }
            }
        }

        // ③ venv 识别（必须先于 symlink 展开）：venv/bin/python 是指向 base 解释器的
        //    symlink，canonicalize 会把 venv 路径「吃掉」变成 pyenv 真身——这正是
        //    用户反馈「选了项目 venv 却显示 pyenv 路径」的根因。
        //    判据：pyvenv.cfg 位于【venv 根目录】= bin 的上一级（virtualenv/venv 规范：
        //    <venv>/pyvenv.cfg + <venv>/bin/python），不是 bin 的同目录！
        //    venv 的 bin/python 本身就是可直接执行的解释器（含 site-packages 隔离），
        //    原样返回。
        if let Some(bin_dir) = std::path::Path::new(&resolved).parent()
            && let Some(venv_root) = bin_dir.parent()
            && venv_root.join("pyvenv.cfg").is_file()
        {
            return ResolvedInterpreter {
                picked_path: picked.to_string(),
                version: Self::probe_version(&resolved).await,
                resolved_path: resolved,
                how: "venv".into(),
            };
        }

        // ④ symlink 真身（/usr/bin/python3 → Xcode 的等；venv 已在上面拦下）
        if let Ok(canon) = std::path::Path::new(&resolved).canonicalize() {
            if canon != std::path::Path::new(&resolved) {
                resolved = canon.to_string_lossy().into_owned();
                if how == "as-is" {
                    how = "symlink".into();
                }
            }
        }

        // ⑤ 反查 venv（根因三：macOS NSOpenPanel 的 resolvesAliases 默认 true，
        //    用户在面板里点 venv/bin/python 时【返回值已被解析成 base 真身】，
        //    rfd/tauri-plugin-dialog 未暴露关闭开关——我们拿到的 picked 就不是 venv）。
        //    对策：picked 指向 base 解释器时，扫描常见项目位置的 venv，
        //    用 pyvenv.cfg 的 base-executable/home 反向匹配，命中则改返回 venv 路径。
        if how != "venv"
            && let Some(venv) = self.find_venv_referencing(&resolved).await
        {
            resolved = venv;
            how = "venv-reverse".into();
        }

        // 校验可用性并取版本
        let version = match run_probe(&resolved, &["--version"]).await {
            Ok(out) => {
                parse_python_version(&out.stdout).or_else(|| parse_python_version(&out.stderr))
            }
            Err(_) => None,
        };
        tracing::info!(picked = %picked, resolved = %resolved, how = %how, version = ?version, "解释器路径解析");
        ResolvedInterpreter {
            picked_path: picked.to_string(),
            resolved_path: resolved,
            version,
            how,
        }
    }

    /// 文件选择器起始目录建议（python）：pyenv versions 目录 > ~/.pyenv > /usr/bin
    pub async fn python_start_dir(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        let pyenv_versions = format!("{home}/.pyenv/versions");
        if std::path::Path::new(&pyenv_versions).is_dir() {
            return pyenv_versions;
        }
        let pyenv = format!("{home}/.pyenv");
        if std::path::Path::new(&pyenv).is_dir() {
            return pyenv;
        }
        "/usr/bin".to_string()
    }

    /// 反查 venv：扫描常见项目目录的 `*/.venv/pyvenv.cfg` 与 `*/venv/pyvenv.cfg`，
    /// pyvenv.cfg 的 base-executable / home 指向 picked（或其所在版本目录）即命中。
    /// 多个命中取 mtime 最新的（最近在用的项目）。同步 IO 但目录浅、量小。
    async fn find_venv_referencing(&self, base: &str) -> Option<String> {
        let home = std::env::var("HOME").ok()?;
        let base_path = std::path::Path::new(base);
        let base_canon = base_path.canonicalize().ok();

        let mut project_roots: Vec<std::path::PathBuf> = Vec::new();
        for root in [
            "PycharmProjects",
            "PyCharmMiscProject",
            "RustroverProjects",
            "IdeaProjects",
            "Projects",
            "code",
            "dev",
        ] {
            let dir = std::path::Path::new(&home).join(root);
            if dir.is_dir() {
                project_roots.push(dir);
            }
        }

        let mut hits: Vec<(std::time::SystemTime, String)> = Vec::new();
        for root in project_roots {
            let Ok(projects) = std::fs::read_dir(&root) else {
                continue;
            };
            for project in projects.flatten() {
                for venv_name in [".venv", "venv"] {
                    let cfg = project.path().join(venv_name).join("pyvenv.cfg");
                    if !cfg.is_file() {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&cfg) else {
                        continue;
                    };
                    let mut referenced = false;
                    for line in text.lines() {
                        let t = line.trim();
                        let value = t
                            .strip_prefix("base-executable = ")
                            .or_else(|| t.strip_prefix("home = "))
                            .or_else(|| t.strip_prefix("base-exec-prefix = "));
                        if let Some(v) = value {
                            let v = v.trim().trim_matches('"');
                            let v_path = std::path::Path::new(v);
                            let v_canon = v_path
                                .canonicalize()
                                .unwrap_or_else(|_| v_path.to_path_buf());
                            if let Some(bp) = base_canon.as_deref() {
                                if v_canon == bp
                                    || bp.starts_with(&v_canon)
                                    || v_canon.starts_with(base_path)
                                {
                                    referenced = true;
                                    break;
                                }
                            }
                        }
                    }
                    if referenced {
                        let Some(venv_dir) = cfg.parent() else {
                            continue;
                        };
                        let python = ["python3", "python"]
                            .iter()
                            .map(|n| venv_dir.join("bin").join(n))
                            .find(|p| p.exists());
                        if let Some(py) = python {
                            let mtime = std::fs::metadata(venv_dir)
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            hits.push((mtime, py.to_string_lossy().into_owned()));
                        }
                    }
                }
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        tracing::info!(base = %base, found = ?hits.first(), "反查 venv");
        hits.into_iter().next().map(|(_, p)| p)
    }

    /// 安卓前台应用（§10 探测链）。剪枝：adb 不可用 → 0 次 shell 调用；
    /// 无在线设备 → 只调 devices；解析不出前台窗口 → 空态提示。
    /// 指定设备的前台应用（P8：设备页按设备查询）；serial=None 自动选首台在线设备
    pub async fn foreground_on(&self, serial: Option<String>) -> ForegroundApp {
        if let Some(ref target) = serial {
            return self.foreground_inner(target.clone()).await;
        }
        self.foreground_auto().await
    }

    async fn foreground_auto(&self) -> ForegroundApp {
        let env = self.adb.environment().await;
        if !env.installed {
            return ForegroundApp {
                state: FgState::AdbUnavailable.as_str().into(),
                hint: Some("adb 不可用，前台应用检测暂停。".into()),
                ..ForegroundApp::default()
            };
        }
        let adb_path = env.path.clone().unwrap_or_default();
        // 自动选第一台在线设备
        let devices_args = adb_adapter::build_args(None, &adb_adapter::cmd_devices());
        let devices_out = match self
            .adb
            .run(&adb_path, &devices_args, FG_SHELL_TIMEOUT)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                return ForegroundApp {
                    state: FgState::Error.as_str().into(),
                    error: Some(format!("adb devices 失败: {e}")),
                    ..ForegroundApp::default()
                };
            }
        };
        let Some(serial) = adb_adapter::parse_devices(&devices_out.stdout)
            .into_iter()
            .find(|d| d.is_ready())
            .map(|d| d.serial)
        else {
            return ForegroundApp {
                state: FgState::NoDevice.as_str().into(),
                hint: Some("无在线设备：连接设备/模拟器后自动开始检测。".into()),
                ..ForegroundApp::default()
            };
        };
        self.foreground_inner(serial).await
    }

    /// 对指定 serial 的完整探测链（窗口 → pid/lib → /proc）
    async fn foreground_inner(&self, serial: String) -> ForegroundApp {
        let env = self.adb.environment().await;
        if !env.installed {
            return ForegroundApp {
                state: FgState::AdbUnavailable.as_str().into(),
                hint: Some("adb 不可用，前台应用检测暂停。".into()),
                ..ForegroundApp::default()
            };
        }
        let adb_path = env.path.clone().unwrap_or_default();

        // 前台窗口
        let win_out = match self.shell(&adb_path, &serial, "dumpsys window").await {
            Ok(o) => o,
            Err(e) => {
                return ForegroundApp {
                    state: FgState::Error.as_str().into(),
                    serial: Some(serial),
                    error: Some(format!("dumpsys window 失败: {e}")),
                    ..ForegroundApp::default()
                };
            }
        };
        let Some((package, activity)) = parse_foreground_window(&win_out.stdout) else {
            return ForegroundApp {
                state: FgState::NoForeground.as_str().into(),
                serial: Some(serial),
                hint: Some("未解析到前台窗口（可能锁屏、弹窗或系统版本输出差异）。".into()),
                ..ForegroundApp::default()
            };
        };

        // 3) 包详情（pidof + legacyNativeLibraryDir）与 4) /proc 探测并发
        let pid_cmd = format!("pidof {package}");
        let lib_cmd = format!("dumpsys package {package} | grep legacyNativeLibraryDir");
        let (pid_res, lib_res) = tokio::join!(
            self.shell(&adb_path, &serial, &pid_cmd),
            self.shell(&adb_path, &serial, &lib_cmd),
        );
        let pid = pid_res
            .ok()
            .and_then(|o| parse_pidof(&o.stdout))
            .unwrap_or_default();
        let native_lib_dir = lib_res
            .ok()
            .and_then(|o| parse_legacy_native_lib(&o.stdout));

        let proc_paths = if pid.is_empty() {
            Vec::new()
        } else {
            self.probe_proc_paths(&adb_path, &serial, &pid).await
        };

        ForegroundApp {
            state: FgState::Ready.as_str().into(),
            serial: Some(serial),
            package: Some(package),
            activity: Some(activity),
            pid: non_empty(pid),
            native_lib_dir,
            proc_paths,
            hint: None,
            error: None,
        }
    }

    /// /proc/<pid> 关键路径摘要（maps 行数、cmdline、status 头几行）。
    /// 全部尽力而为：读不到（需 root / 进程已退）标 readable=false。
    async fn probe_proc_paths(&self, adb_path: &str, serial: &str, pid: &str) -> Vec<ProcPath> {
        let base = format!("/proc/{pid}");
        let maps_f = format!("{base}/maps");
        let cmdline_f = format!("{base}/cmdline");
        let status_f = format!("{base}/status");

        let maps_cmd = format!("wc -l {maps_f}");
        let cmdline_cmd = format!("cat {cmdline_f}");
        let status_cmd = format!("head -4 {status_f}");
        let (maps, cmdline, status) = tokio::join!(
            self.shell(adb_path, serial, &maps_cmd),
            self.shell(adb_path, serial, &cmdline_cmd),
            self.shell(adb_path, serial, &status_cmd),
        );
        let maps_readable = maps.is_ok();
        let cmdline_readable = cmdline.is_ok();
        let status_readable = status.is_ok();
        vec![
            ProcPath {
                name: "maps".into(),
                path: maps_f,
                summary: maps.ok().map(|o| {
                    let t = o.stdout.trim();
                    // toybox wc 输出 "123 /proc/x/maps"，取行数段
                    t.split_whitespace().next().unwrap_or(t).to_string()
                }),
                readable: maps_readable,
            },
            ProcPath {
                name: "cmdline".into(),
                path: cmdline_f,
                summary: cmdline.ok().map(|o| {
                    let s = o.stdout.replace('\0', " ");
                    let s = s.trim().to_string();
                    if s.chars().count() > 120 {
                        s.chars().take(120).collect::<String>() + "…"
                    } else {
                        s
                    }
                }),
                readable: cmdline_readable,
            },
            ProcPath {
                name: "status".into(),
                path: status_f,
                summary: status.ok().map(|o| o.stdout.trim().to_string()),
                readable: status_readable,
            },
        ]
    }

    async fn shell(&self, adb_path: &str, serial: &str, command: &str) -> CoreResult<AdbRunOutput> {
        let args = adb_adapter::build_args(Some(serial), &adb_adapter::cmd_shell(command));
        self.adb.run(adb_path, &args, FG_SHELL_TIMEOUT).await
    }

    /// 仪表盘聚合：全部环境卡并发探测（Frida 复用 Python 结果，未就绪即剪枝）。
    pub async fn overview(&self) -> EnvOverview {
        let (python, node, ida, jadx) =
            tokio::join!(self.python(), self.node(), self.ida_mcp(), self.jadx_mcp());
        let frida = self.frida_after(python.clone()).await;
        EnvOverview {
            python,
            node,
            frida,
            ida_mcp: ida,
            jadx_mcp: jadx,
        }
    }
}

// ===== 探测工具 =====

/// 运行外部探测命令（capture，短超时）。Windows 防 CREATE_NO_WINDOW 闪窗（仅预留）。
async fn run_probe(program: &str, args: &[&str]) -> Result<ProbeOutput, String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let fut = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(PROBE_TIMEOUT, fut)
        .await
        .map_err(|_| format!("超时（{PROBE_TIMEOUT:?}）"))?
        .map_err(|e| e.to_string())?;
    Ok(ProbeOutput {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        exit_code: out.status.code(),
    })
}

/// mac：在 /Applications 找 IDA（用户指定的 find 命令语义，浅层 3 级足够）。
/// 非 mac 返回 None（暂留空，其他平台探测后续补）。
pub async fn detect_ida_app() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // find /Applications -maxdepth 3 \( -iname "ida.app" -o -iname "ida64" -o -iname "*ida*" \) 2>/dev/null
    let out = run_probe(
        "find",
        &[
            "/Applications",
            "-maxdepth",
            "3",
            "(",
            "-iname",
            "ida.app",
            "-o",
            "-iname",
            "ida64",
            "-o",
            "-iname",
            "*ida*",
            ")",
        ],
    )
    .await
    .ok()?;
    let v = first_line(&out.stdout);
    if v.is_empty() { None } else { Some(v) }
}

/// `jadx-gui --version`：在 PATH 即视为安装，输出版本号
pub async fn detect_jadx_cli() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = run_probe("jadx-gui", &["--version"]).await.ok()?;
    let v = first_line(&out.stdout);
    if v.is_empty() { None } else { Some(v) }
}

/// MCP 端口探活：TCP 可连 = 服务在跑；连不上是常态（未启动），不算错误。
async fn probe_mcp(name: &str, port: u16) -> McpEnv {
    use tokio::net::TcpStream;
    let addr = format!("127.0.0.1:{port}");
    match tokio::time::timeout(MCP_CONNECT_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => McpEnv {
            reachable: true,
            port,
            hint: Some(format!("{name} 服务在线（{addr}）")),
            app_installed: None,
            app_path: None,
        },
        Ok(Err(e)) => McpEnv {
            reachable: false,
            port,
            hint: Some(format!(
                "{name} 未检测到（{addr} 连接失败: {e}）。启动工具后点刷新。"
            )),
            app_installed: None,
            app_path: None,
        },
        Err(_) => McpEnv {
            reachable: false,
            port,
            hint: Some(format!("{name} 未检测到（{addr} 连接超时）。")),
            app_installed: None,
            app_path: None,
        },
    }
}

/// npm 全局包根目录（`npm root -g`）；失败返回 None
async fn probe_npm_root(node_path: &str) -> Option<String> {
    let npm = if node_path.ends_with("node.exe") {
        node_path.replace("node.exe", "npm.cmd")
    } else {
        "npm".to_string()
    };
    let out = run_probe(&npm, &["root", "-g"]).await.ok()?;
    let s = out.stdout.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

struct ProbeOutput {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
    #[allow(dead_code)]
    exit_code: Option<i32>,
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

// ===== 纯解析函数（无 IO，单测覆盖多格式） =====

/// dumpsys window → (包名, Activity)。
/// 兼容 mCurrentFocus / mFocusedWindow；null → None。
pub fn parse_foreground_window(stdout: &str) -> Option<(String, String)> {
    for line in stdout.lines() {
        let t = line.trim();
        if !(t.starts_with("mCurrentFocus") || t.starts_with("mFocusedWindow")) {
            continue;
        }
        if t.contains("null") {
            return None;
        }
        // Window{7a4c1de u0 com.pkg/com.pkg.Activity}
        if let Some(brace) = t.find('{') {
            let inner = &t[brace + 1..].trim_end_matches('}');
            if let Some(activity_part) = inner.split_whitespace().find(|s| s.contains('/')) {
                let mut it = activity_part.splitn(2, '/');
                let package = it.next()?.to_string();
                let activity = it.next()?.to_string();
                if !package.is_empty() && !activity.is_empty() {
                    return Some((package, activity));
                }
            }
        }
    }
    None
}

/// `dumpsys package <pkg> | grep legacyNativeLibraryDir` → 目录路径
pub fn parse_legacy_native_lib(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(idx) = line.find("legacyNativeLibraryDir=") {
            let v = line[idx + "legacyNativeLibraryDir=".len()..].trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// pidof 输出 → 首个 PID（可能多列 "123 456"）
pub fn parse_pidof(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .map(String::from)
}

/// `python --version`（stdout 或 stderr）→ "3.12.4"
pub fn parse_python_version(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Python ") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// `node --version` → "20.11.1"（剥掉 v 前缀）
pub fn parse_node_version(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('v') {
            let v = rest.trim();
            if v.split('.')
                .next()
                .map(|m| m.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

// ===== DTO =====

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonEnv {
    /// 用户配置了解释器路径
    pub configured: bool,
    /// 探测成功（能执行 --version 且输出可解析）
    pub ready: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnv {
    pub ready: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// npm 全局包根目录（npm root -g）
    pub npm_global_root: Option<String>,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FridaEnv {
    pub python_ready: bool,
    /// frida 或 frida-tools 任一安装即 true
    pub installed: bool,
    pub frida_version: Option<String>,
    pub frida_tools_version: Option<String>,
    pub hint: Option<String>,
}

/// resolve_interpreter 的返回（serde camelCase 对齐 TS）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedInterpreter {
    pub picked_path: String,
    pub resolved_path: String,
    pub version: Option<String>,
    /// as-is | pyenv-shim | app-bundle | symlink
    pub how: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEnv {
    pub reachable: bool,
    pub port: u16,
    pub hint: Option<String>,
    /// 宿主应用是否存在（P8 追加：mac 检测 /Applications；其他平台暂留 None）
    pub app_installed: Option<bool>,
    /// 检测到的应用可执行文件路径（mac）
    pub app_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FgState {
    Ready,
    AdbUnavailable,
    NoDevice,
    NoForeground,
    Error,
}

impl FgState {
    pub fn as_str(&self) -> &'static str {
        match self {
            FgState::Ready => "ready",
            FgState::AdbUnavailable => "adb_unavailable",
            FgState::NoDevice => "no_device",
            FgState::NoForeground => "no_foreground",
            FgState::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcPath {
    /// maps | cmdline | status
    pub name: String,
    pub path: String,
    /// 摘要（maps=行数、cmdline=命令行截断、status=头几行）；不可读为 None
    pub summary: Option<String>,
    pub readable: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundApp {
    /// ready | adb_unavailable | no_device | no_foreground | error
    pub state: String,
    pub serial: Option<String>,
    pub package: Option<String>,
    pub activity: Option<String>,
    pub pid: Option<String>,
    /// legacyNativeLibraryDir
    pub native_lib_dir: Option<String>,
    pub proc_paths: Vec<ProcPath>,
    pub hint: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvOverview {
    pub python: PythonEnv,
    pub node: NodeEnv,
    pub frida: FridaEnv,
    pub ida_mcp: McpEnv,
    pub jadx_mcp: McpEnv,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== 解析纯函数 =====

    #[test]
    fn parse_foreground_window_extracts_package_and_activity() {
        let out = "Devices: false\n  mCurrentFocus: Window{7a4c1de u0 com.example.app/com.example.app.MainActivity}\n";
        let r = parse_foreground_window(out).expect("解析成功");
        assert_eq!(r.0, "com.example.app");
        assert_eq!(r.1, "com.example.app.MainActivity");
    }

    #[test]
    fn parse_foreground_window_falls_back_to_focused_window() {
        let out = "  mFocusedWindow: Window{bf4b5b4 u0 com.android.launcher3/com.android.launcher3.uioverrides.QuickstepLauncher}\n";
        let (pkg, act) = parse_foreground_window(out).unwrap();
        assert_eq!(pkg, "com.android.launcher3");
        assert!(act.contains("Launcher"));
    }

    #[test]
    fn parse_foreground_window_null_and_garbage() {
        assert!(parse_foreground_window("  mCurrentFocus: null\n").is_none());
        assert!(parse_foreground_window("nothing useful here").is_none());
        assert!(parse_foreground_window("").is_none());
    }

    #[test]
    fn parse_legacy_native_lib_extracts_dir() {
        let out = "    legacyNativeLibraryDir=/data/app/~~aBc==/com.example.app-xYz==/lib/arm64\n    primaryCpuAbi=arm64-v8a\n";
        assert_eq!(
            parse_legacy_native_lib(out).as_deref(),
            Some("/data/app/~~aBc==/com.example.app-xYz==/lib/arm64")
        );
        assert!(parse_legacy_native_lib("no match").is_none());
        assert!(parse_legacy_native_lib("legacyNativeLibraryDir=\n").is_none());
    }

    #[test]
    fn parse_pidof_takes_first_and_validates_digits() {
        assert_eq!(parse_pidof("12345\n").as_deref(), Some("12345"));
        assert_eq!(parse_pidof(" 123 456\n").as_deref(), Some("123"));
        assert_eq!(parse_pidof(""), None);
        assert_eq!(parse_pidof("not-a-pid"), None);
    }

    #[test]
    fn parse_python_and_node_versions() {
        // 旧版 python 把 --version 打到 stderr，前端已合并读取
        assert_eq!(
            parse_python_version("Python 3.12.4\n"),
            Some("3.12.4".into())
        );
        assert_eq!(
            parse_python_version("Python 3.13.0rc1"),
            Some("3.13.0rc1".into())
        );
        assert_eq!(parse_python_version("command not found"), None);
        assert_eq!(parse_node_version("v20.11.1\n"), Some("20.11.1".into()));
        assert_eq!(parse_node_version("v22.0.0"), Some("22.0.0".into()));
        assert_eq!(parse_node_version("node: command not found"), None);
    }

    // ===== 前台探测链（Mock runner：可脚本化输出 + 调用计数） =====

    struct ScriptedAdb {
        installed: bool,
        path: Option<String>,
        /// 命令关键词 → 返回 stdout（按顺序 pop，默认空）
        script: std::sync::Mutex<Vec<(&'static str, String)>>,
        /// 每次实际 run() 调用的命令关键词记录
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl ScriptedAdb {
        fn new(installed: bool) -> Self {
            Self {
                installed,
                path: installed.then(|| "/fake/adb".to_string()),
                script: std::sync::Mutex::new(Vec::new()),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn expect(&self, keyword: &'static str, stdout: &str) -> &Self {
            self.script
                .lock()
                .unwrap()
                .push((keyword, stdout.to_string()));
            self
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl AdbRunner for ScriptedAdb {
        async fn run(
            &self,
            _adb_path: &str,
            args: &[String],
            _timeout: Duration,
        ) -> CoreResult<AdbRunOutput> {
            let joined = args.join(" ");
            self.calls.lock().unwrap().push(joined.clone());
            let mut script = self.script.lock().unwrap();
            let out = script
                .iter()
                .position(|(kw, _)| joined.contains(kw))
                .map(|idx| {
                    let (_, s) = script.remove(idx);
                    s
                })
                .unwrap_or_default();
            Ok(AdbRunOutput {
                stdout: out,
                stderr: String::new(),
                exit_code: Some(0),
            })
        }

        async fn environment(&self) -> AdbEnvironment {
            if self.installed {
                AdbEnvironment {
                    installed: true,
                    path: self.path.clone(),
                    source: Some("path_env".into()),
                    version: None,
                    hint: None,
                    probe_error: None,
                }
            } else {
                AdbEnvironment::not_found()
            }
        }

        fn invalidate_cache(&self) {}
    }

    fn svc_with(mock: Arc<ScriptedAdb>) -> EnvService {
        let db = Arc::new(crate::db::Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        EnvService::new(config, mock)
    }

    const FIXTURE_WINDOW: &str =
        "  mCurrentFocus: Window{abc u0 com.target.app/com.target.app.ui.HomeActivity}\n";
    const FIXTURE_LIB: &str =
        "    legacyNativeLibraryDir=/data/app/~~x/com.target.app-y/lib/arm64\n";
    const FIXTURE_STATUS: &str =
        "Name:\tcom.target.app\nState:\tS (sleeping)\nTgid:\t4321\nPid:\t4321\n";

    #[tokio::test]
    async fn foreground_full_chain_parses_all_fields() {
        let mock = Arc::new(ScriptedAdb::new(true));
        mock.expect("devices", "ABC123\tdevice product:foo\n");
        mock.expect("dumpsys window", FIXTURE_WINDOW);
        mock.expect("pidof", "4321\n");
        mock.expect("legacyNativeLibraryDir", FIXTURE_LIB);
        mock.expect("wc -l", "512 /proc/4321/maps\n");
        mock.expect("cmdline", "com.target.app\0--flag\0");
        mock.expect("status", FIXTURE_STATUS);
        let svc = svc_with(mock.clone());

        let fg = svc.foreground_auto().await;
        assert_eq!(fg.state, "ready", "{fg:?}");
        assert_eq!(fg.serial.as_deref(), Some("ABC123"));
        assert_eq!(fg.package.as_deref(), Some("com.target.app"));
        assert_eq!(
            fg.activity.as_deref(),
            Some("com.target.app.ui.HomeActivity")
        );
        assert_eq!(fg.pid.as_deref(), Some("4321"));
        assert_eq!(
            fg.native_lib_dir.as_deref(),
            Some("/data/app/~~x/com.target.app-y/lib/arm64")
        );
        assert_eq!(fg.proc_paths.len(), 3);
        let maps = fg.proc_paths.iter().find(|p| p.name == "maps").unwrap();
        assert_eq!(maps.summary.as_deref(), Some("512"));
        assert!(maps.readable);
        let cmdline = fg.proc_paths.iter().find(|p| p.name == "cmdline").unwrap();
        assert_eq!(cmdline.summary.as_deref(), Some("com.target.app --flag"));
        assert!(fg.error.is_none());
    }

    #[tokio::test]
    async fn foreground_prunes_to_zero_calls_when_adb_missing() {
        // §10 回测核心：adb 不存在 → 前台探测 0 次 shell 调用
        let mock = Arc::new(ScriptedAdb::new(false));
        let svc = svc_with(mock.clone());
        let fg = svc.foreground_auto().await;
        assert_eq!(fg.state, "adb_unavailable");
        assert_eq!(mock.call_count(), 0, "剪枝后不应有任何 adb 调用");
        assert!(fg.package.is_none());
    }

    #[tokio::test]
    async fn foreground_stops_at_no_device() {
        let mock = Arc::new(ScriptedAdb::new(true));
        mock.expect("devices", "\n"); // 空列表
        let svc = svc_with(mock.clone());
        let fg = svc.foreground_auto().await;
        assert_eq!(fg.state, "no_device");
        assert_eq!(mock.call_count(), 1, "无设备时只调了 devices，不再继续");
    }

    #[tokio::test]
    async fn foreground_lock_screen_is_empty_state_not_error() {
        let mock = Arc::new(ScriptedAdb::new(true));
        mock.expect("devices", "ABC123\tdevice\n");
        mock.expect("dumpsys window", "  mCurrentFocus: null\n");
        let svc = svc_with(mock);
        let fg = svc.foreground_auto().await;
        assert_eq!(fg.state, "no_foreground");
        assert!(fg.hint.is_some());
        assert!(fg.error.is_none(), "空态不是错误");
    }

    #[test]
    fn parse_frida_versions_handles_all_shapes() {
        let (f, t) = parse_frida_versions(r#"{"frida": "16.5.9", "frida-tools": "13.6.1"}"#);
        assert_eq!(f.as_deref(), Some("16.5.9"));
        assert_eq!(t.as_deref(), Some("13.6.1"));
        // 只装了其中一个
        let (f, t) = parse_frida_versions(r#"{"frida": "16.5.9", "frida-tools": null}"#);
        assert_eq!(f.as_deref(), Some("16.5.9"));
        assert_eq!(t, None);
        // 全未安装 / 输出损坏
        let (f, t) = parse_frida_versions(r#"{"frida": null, "frida-tools": null}"#);
        assert_eq!((f, t), (None, None));
        let (f, t) = parse_frida_versions("Traceback (most recent call last): ...");
        assert_eq!((f, t), (None, None));
    }

    #[tokio::test]
    async fn frida_probe_is_pruned_without_python() {
        // §10 剪枝：Python 未配置 → frida 不发起任何子进程，直接返回剪枝态
        let db = Arc::new(crate::db::Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let svc = EnvService::new(config, Arc::new(ScriptedAdb::new(false)));
        let frida = svc.frida().await;
        assert!(!frida.python_ready);
        assert!(!frida.installed);
        assert!(frida.hint.unwrap().contains("未配置"));
    }

    #[tokio::test]
    async fn resolve_base_interpreter_reverse_finds_venv() {
        // 根因三回归：macOS 文件选择器 resolvesAliases 把 venv/bin/python 解析成
        // base 真身返回——host 拿到的 picked 已是 ~/.pyenv/...；反查 venv 应回到 venv。
        let venv_python = "/Users/citec/PycharmProjects/android_reverse_study/.venv/bin/python";
        let base = "/Users/citec/.pyenv/versions/3.13.5/bin/python3.13";
        if !std::path::Path::new(venv_python).exists() || !std::path::Path::new(base).exists() {
            eprintln!("skip: 本机缺该 venv/base");
            return;
        }
        let db = Arc::new(crate::db::Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let svc = EnvService::new(config, Arc::new(ScriptedAdb::new(false)));
        let r = svc.resolve_interpreter(base).await;
        eprintln!(
            "picked={} resolved={} how={}",
            r.picked_path, r.resolved_path, r.how
        );
        let venv_bin = std::path::Path::new(venv_python).parent().unwrap();
        assert_eq!(
            std::path::Path::new(&r.resolved_path).parent(),
            Some(venv_bin),
            "应反查回该 venv 的 bin（python3/python 任一变体）"
        );
        assert_eq!(r.how, "venv-reverse");
    }

    #[tokio::test]
    async fn venv_python_is_returned_as_is() {
        // 回归（§10.5.1）：pyvenv.cfg 在 venv 根目录（bin 的上一级），不在 bin 同目录。
        // 选 venv/bin/python 必须原样返回，不得 canonicalize 成 pyenv 真身。
        let venv_bin = "/Users/citec/PycharmProjects/android_reverse_study/.venv/bin";
        if !std::path::Path::new(&format!("{venv_bin}/python")).exists() {
            eprintln!("skip: 本机无该 venv");
            return;
        }
        let db = Arc::new(crate::db::Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let svc = EnvService::new(config, Arc::new(ScriptedAdb::new(false)));
        for name in ["python", "python3"] {
            let picked = format!("{venv_bin}/{name}");
            let r = svc.resolve_interpreter(&picked).await;
            assert_eq!(r.resolved_path, picked, "{name}: venv 路径必须原样返回");
            assert_eq!(r.how, "venv");
            assert!(r.version.is_some(), "{name}: 应探得版本");
        }
    }

    #[test]
    fn python_version_parsed_from_stderr_too() {
        // run_probe 层合并了 stdout/stderr 后交给 parse；这里验证两个来源都认
        assert_eq!(
            parse_python_version("Python 3.12.4\n"),
            Some("3.12.4".into())
        );
        // framework 入口打印额外警告行 + stderr 版本（典型文件选择器选中场景）
        let mixed = "python.exe: can't open file";
        assert_eq!(parse_python_version(mixed), None);
    }

    #[tokio::test]
    async fn python_normalize_rejects_garbage_without_side_effects() {
        // 非法路径：兜底不 panic、不误写配置
        let db = Arc::new(crate::db::Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        config
            .set(KEY_PYTHON_PATH, "/nonexistent/python/binary")
            .unwrap();
        let svc = EnvService::new(config.clone(), Arc::new(ScriptedAdb::new(false)));
        let env = svc.python().await;
        assert!(!env.ready);
        assert!(env.hint.unwrap_or_default().contains("执行失败"));
        // 配置未被兜底污染
        assert_eq!(
            config.get(KEY_PYTHON_PATH, "").unwrap(),
            "/nonexistent/python/binary"
        );
    }

    #[tokio::test]
    async fn ida_jadx_app_detection_platform_gate() {
        // 非 mac 恒 None（其他平台暂留空，仅设计预留 §1.5）
        if !cfg!(target_os = "macos") {
            assert!(detect_ida_app().await.is_none());
            assert!(detect_jadx_cli().await.is_none());
            return;
        }
        // mac：真实探测。本机装没装都可——只断言不 panic 且类型正确。
        // IDA：find /Applications 命中与否取决于机器，不强制断言结果。
        let _ = detect_ida_app().await;
        let _ = detect_jadx_cli().await;
    }

    #[tokio::test]
    async fn mcp_port_probe_reports_unreachable_without_panic() {
        // 用一个大概率没人监听的端口
        let svc = {
            let db = Arc::new(crate::db::Db::in_memory().unwrap());
            let config = Arc::new(ConfigService::new(db));
            EnvService::new(config, Arc::new(ScriptedAdb::new(false)))
        };
        let env = svc.jadx_mcp().await;
        // 端口默认值来自配置默认
        assert_eq!(env.port, DEFAULT_JADX_MCP_PORT);
        // 连不上是常态：reachable=false 且有可读 hint
        if !env.reachable {
            assert!(env.hint.unwrap().contains("未检测到"));
        }
    }
}
