//! 翻译服务（UI-6 第二层的可选环节）。
//!
//! 三条纪律都是用户明确点过名的，代码里逐条钉住：
//! ① **API key 只存本机**（ConfigService 的本地 SQLite）：不下发设备、不进 adb 命令行、
//!    不进日志、不进 Git；界面只拿得到"后 4 位 + 有没有配"，永远拿不到原文；
//! ② 请求只发给**用户自己填的**地址，本工具不替用户把内容送到任何第三方；
//! ③ 没配/失败 → 保留原文并说明原因，**绝不阻塞探测结果本身**。
//!
//! 为什么用系统 curl 而不是引一个 HTTP crate：这个仓库为一个可选的"读完 help 再翻译"
//! 拖进 tls + http + 一整棵依赖树不划算，而 curl 在 macOS/Linux 上都是现成的，
//! TLS 还走系统实现。代价是多一次进程调用，换来的是依赖不变重。
//!
//! ⚠️ key 绝不进 argv：`ps` 能看见命令行。走 `-H @文件`（curl 7.55+ 支持从文件读请求头），
//! 请求体同样走 `--data-binary @文件`。两个临时文件 0600，无论成败都立刻删。

use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::core::error::{CoreError, CoreResult};
use crate::services::config_service::{
    ConfigService, KEY_AI_API_KEY, KEY_AI_BASE_URL, KEY_AI_ENABLED, KEY_AI_MODEL,
};

/// 送给接口的最大字符数：help 全文可能几十 KB，而翻译按字符计费。
/// 截断必须在界面上说明，不能让用户以为整篇都翻了。
pub const MAX_TRANSLATE_CHARS: usize = 12_000;
/// 一次翻译的硬超时：这是"点一下按钮"的动效，等久了该报错而不是一直挂着。
const TRANSLATE_TIMEOUT_SECS: u64 = 60;

/// 给界面看的配置视图：**不含 key 本体**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigView {
    pub base_url: String,
    pub model: String,
    pub enabled: bool,
    pub has_api_key: bool,
    /// key 的后 4 位（没配则空串）。界面显示的就该只有这个。
    pub key_tail: String,
}

/// 注意没有 `Debug`：结构里能取到 API key，派生一个把字段打进日志的能力是纪律①的漏洞。
pub struct AiService {
    config: std::sync::Arc<ConfigService>,
}

impl AiService {
    pub fn new(config: std::sync::Arc<ConfigService>) -> Self {
        Self { config }
    }

    /// 读配置。key 走 `get_secret`：它不在 `ALLOWED_KEYS` 里，
    /// 用普通 `get` 会被"未注册的配置键"挡掉（这层挡的其实是 snapshot，不是读）。
    fn get(&self, key: &str) -> String {
        let raw = if key == KEY_AI_API_KEY {
            self.config.get_secret(key, "").unwrap_or_default()
        } else {
            self.config.get(key, "").unwrap_or_default()
        };
        raw.trim().to_owned()
    }

    pub fn view(&self) -> AiConfigView {
        let key = self.get(KEY_AI_API_KEY);
        AiConfigView {
            base_url: self.get(KEY_AI_BASE_URL),
            model: self.get(KEY_AI_MODEL),
            enabled: self.get(KEY_AI_ENABLED) == "true",
            has_api_key: !key.is_empty(),
            key_tail: tail(&key, 4),
        }
    }

    /// 保存配置。`api_key` 为 `None` = 不动它，空串 = 清掉。
    pub fn save(
        &self,
        base_url: &str,
        model: &str,
        enabled: bool,
        api_key: Option<&str>,
    ) -> CoreResult<AiConfigView> {
        let base_url = base_url.trim();
        if !base_url.is_empty() {
            validate_endpoint(base_url)?;
        }
        if enabled && (base_url.is_empty() || api_key.map(str::trim) == Some("")) {
            // 开着翻译却没地址或明确清空了 key：这是配错，不是"没配"
            return Err(CoreError::InvalidInput(
                "开启翻译需要接口地址与 API key；只想看原文就关掉翻译".to_string(),
            ));
        }
        self.config.set(KEY_AI_BASE_URL, base_url)?;
        self.config.set(KEY_AI_MODEL, model.trim())?;
        self.config
            .set(KEY_AI_ENABLED, if enabled { "true" } else { "false" })?;
        if let Some(key) = api_key {
            // 只走 secret 通道：这个键永远不进 snapshot，也就永远不被前端整包拉走
            self.config.set_secret(KEY_AI_API_KEY, key.trim())?;
        }
        Ok(self.view())
    }

    /// 把文本翻成目标语言。返回 `Err` 的每一种情况，界面都必须保留原文。
    pub async fn translate(
        &self,
        text: &str,
        target_lang: &str,
        source_lang: Option<&str>,
    ) -> CoreResult<TranslateOutcome> {
        let view = self.view();
        if !view.enabled {
            return Err(CoreError::InvalidInput("翻译没有开启".to_string()));
        }
        if view.base_url.is_empty() || view.model.is_empty() {
            return Err(CoreError::InvalidInput(
                "还没配翻译接口：填接口地址与模型；只想看原文就把翻译关掉".to_string(),
            ));
        }
        if !view.has_api_key {
            return Err(CoreError::InvalidInput(
                "配了接口但没有 API key".to_string(),
            ));
        }
        let key = self.get(KEY_AI_API_KEY);
        let url = completions_url(&view.base_url);
        let (body, truncated) = build_request_body(text, target_lang, source_lang, &view.model)
            .ok_or_else(|| CoreError::InvalidInput("没有内容要翻译".to_string()))?;

        // 文件名带随机段：并发点两次翻译不该互相覆盖对方的请求体
        let stem = format!("artool-ai-{:016x}", rand_u64());
        let body_path = std::env::temp_dir().join(format!("{stem}.json"));
        let header_path = std::env::temp_dir().join(format!("{stem}.headers"));
        let written = write_private(&body_path, body.as_bytes())
            .and_then(|()| write_private(&header_path, header_file_content(&key).as_bytes()));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&body_path);
            let _ = std::fs::remove_file(&header_path);
            return Err(error);
        }

        let args = curl_args(&url, &header_path, &body_path);
        let outcome = tokio::process::Command::new("curl")
            .args(&args)
            .output()
            .await;
        // 无论成败，本机这两个文件都不留：里面有 key 与待译全文
        let _ = std::fs::remove_file(&header_path);
        let _ = std::fs::remove_file(&body_path);

        let output =
            outcome.map_err(|error| CoreError::Internal(format!("调用本机 curl 失败: {error}")))?;
        if !output.status.success() {
            // curl 的 stderr 可能带 URL，但不带 key（key 只在请求头文件里）
            return Err(CoreError::Internal(format!(
                "接口调用失败（退出码 {:?}）：{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let translated = extract_message(&String::from_utf8_lossy(&output.stdout))?;
        Ok(TranslateOutcome {
            text: translated,
            source_truncated: truncated,
            sent_chars: text.chars().count().min(MAX_TRANSLATE_CHARS),
        })
    }
}

/// 翻译结果 + 界面上必须一起说清的两个事实
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateOutcome {
    pub text: String,
    /// 原文超过上限时被截断：译文自然只覆盖了送出去的那一段
    pub source_truncated: bool,
    /// 实际送出去的字符数（界面显示"已送出 N 字"）
    pub sent_chars: usize,
}

fn tail(value: &str, take: usize) -> String {
    value
        .chars()
        .rev()
        .take(take)
        .collect::<Vec<_>>()
        .iter()
        .rev()
        .cloned()
        .collect()
}

/// 只认 https，或本机的 http：把 key 明文发出去是不可接受的失败模式。
pub fn validate_endpoint(base_url: &str) -> CoreResult<()> {
    let lower = base_url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let rest = lower.strip_prefix("http://").unwrap_or_default();
    let is_local = rest.starts_with("127.0.0.1")
        || rest.starts_with("localhost")
        || rest.starts_with("[::1]")
        || rest.starts_with("::1");
    if lower.starts_with("http://") && is_local {
        return Ok(());
    }
    Err(CoreError::InvalidInput(
        "接口地址要用 https（本机服务可以用 http://127.0.0.1…）；明文 http 会把 key 裸发出去"
            .to_string(),
    ))
}

/// 把"用户填的基址"归一到 OpenAI 兼容端点。
pub fn completions_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        return trimmed.to_owned();
    }
    if trimmed.ends_with("/v1") {
        return format!("{trimmed}/chat/completions");
    }
    format!("{trimmed}/v1/chat/completions")
}

/// curl 的参数表：**只出现文件路径**，key 与正文都不在这里。
///
/// 单测会拿一把假 key 打这条，断言整张参数表里找不到 "Bearer" 和 key 本体——
/// "别把 key 放进 argv"这种纪律不钉成测试，早晚会被人顺手改回 `-H "Bearer …"`。
pub fn curl_args(url: &str, header_file: &Path, body_file: &Path) -> Vec<String> {
    vec![
        "--silent".into(),
        "--show-error".into(),
        "--fail-with-body".into(),
        "--max-time".into(),
        TRANSLATE_TIMEOUT_SECS.to_string(),
        "-X".into(),
        "POST".into(),
        "-H".into(),
        format!("@{}", header_file.display()),
        "-H".into(),
        "Content-Type: application/json".into(),
        "--data-binary".into(),
        format!("@{}", body_file.display()),
        url.to_owned(),
    ]
}

/// 请求头文件内容：curl 从文件读 header，一行一条。
fn header_file_content(api_key: &str) -> String {
    format!(
        "Authorization: Bearer {}\r\n",
        api_key.replace(['\r', '\n'], "")
    )
}

/// OpenAI 兼容的 chat/completions 请求体。
///
/// 提示词只做两件事：只输出译文、把命令与参数名原样留着。
/// help 文本里 `-h`、`--help`、路径这些翻了就成了废信息，这是这类内容最容易翻坏的地方。
pub fn build_request_body(
    text: &str,
    target_lang: &str,
    source_lang: Option<&str>,
    model: &str,
) -> Option<(String, bool)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut chars = trimmed.chars();
    let kept: String = chars.by_ref().take(MAX_TRANSLATE_CHARS).collect();
    let truncated = chars.next().is_some();
    let source_note = source_lang
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("自动判断（多为英文）");
    let system = "你是技术文档翻译。只输出译文本身：不要解释、不要加代码块围栏、不要添删段落。\
        命令、参数名（如 -h、--help）、路径、占位符（如 <file>）、程序名与选项前的符号\
        一律原样保留；每行的结构与缩进保持不变。";
    let user = format!("目标语言：{target_lang}\n原文语言：{source_note}\n待译内容：\n{kept}");
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "stream": false,
    });
    Some((body.to_string(), truncated))
}

/// 从响应里取译文；接口报错时把它的 message 带出来（不含 key）。
fn extract_message(raw: &str) -> CoreResult<String> {
    let value: serde_json::Value = serde_json::from_str(raw.trim()).map_err(|_| {
        CoreError::Internal(format!(
            "接口返回的不是 JSON（前 200 字）：{}",
            raw.chars().take(200).collect::<String>()
        ))
    })?;
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
    {
        return Err(CoreError::Internal(format!("接口返回错误：{message}")));
    }
    value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|first| first.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Internal("接口响应里没有译文字段".to_string()))
}

/// 0600 落盘：待译全文与 key 都不该被同机其它用户/进程读到。
fn write_private(path: &Path, content: &[u8]) -> CoreResult<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(CoreError::Io)?;
    file.write_all(content).map_err(CoreError::Io)?;
    file.sync_data().ok();
    Ok(())
}

/// 只做"别和并发任务撞名"用，不需要密码学强度。
fn rand_u64() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    nanos ^ counter.rotate_left(17)
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::services::config_service::ConfigService;
    use std::sync::Arc;

    #[test]
    fn endpoint_normalizes_the_common_forms() {
        assert_eq!(
            completions_url("https://api.example.com/v1"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url("https://api.example.com/v1/"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url("https://api.example.com"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url("https://gw.example.com/v1/chat/completions"),
            "https://gw.example.com/v1/chat/completions",
            "已经填到端点的不该被再拼一次"
        );
    }

    #[test]
    fn plaintext_http_is_refused_unless_it_is_local() {
        assert!(validate_endpoint("https://api.openai.com/v1").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_endpoint("http://localhost:11434/v1").is_ok());
        assert!(
            validate_endpoint("http://some-relay.example.com/v1").is_err(),
            "明文 http 会把 key 裸发出去"
        );
        assert!(validate_endpoint("ftp://x/v1").is_err());
    }

    /// 纪律①的机械化：key 永远不进 argv（`ps` 看得到命令行）。
    #[test]
    fn api_key_never_reaches_the_command_line() {
        let args = curl_args(
            "https://api.example.com/v1/chat/completions",
            Path::new("/tmp/artool-ai-1.headers"),
            Path::new("/tmp/artool-ai-1.json"),
        );
        let joined = args.join(" ");
        assert!(!joined.contains("Bearer"), "命令行里不该出现认证头");
        assert!(
            joined.contains("@/tmp/artool-ai-1.headers"),
            "请求头必须走文件：{joined}"
        );
        assert!(joined.contains("@/tmp/artool-ai-1.json"));
        // 请求头文件本身才带 key，且只带一行
        let header = header_file_content("sk-abcdef123456");
        assert_eq!(header, "Authorization: Bearer sk-abcdef123456\r\n");
        // 换行注入：key 里塞第二条 header 也不会多出一行
        assert_eq!(
            header_file_content("sk-x\r\nX-Evil: 1").lines().count(),
            1,
            "key 里的换行必须被吃掉，不然能注入请求头"
        );
    }

    #[test]
    fn body_is_capped_and_says_so() {
        let long = "啊".repeat(MAX_TRANSLATE_CHARS + 10);
        let (body, truncated) = build_request_body(&long, "中文", None, "m").unwrap();
        assert!(truncated, "超上限必须报被截断，不能假装全翻了");
        let value: serde_json::Value = serde_json::from_str(&body).expect("请求体必须是 JSON");
        assert_eq!(value["model"], "m");
        assert_eq!(value["stream"], false);
        assert_eq!(value["messages"].as_array().unwrap().len(), 2);
        assert!(
            value["messages"][1]["content"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= MAX_TRANSLATE_CHARS + 64,
            "送出去的正文要受上限约束"
        );
        let short = "Usage: toybox [-h] <file>";
        let (body, truncated) = build_request_body(short, "中文", Some("en"), "m").unwrap();
        assert!(!truncated);
        assert!(body.contains("目标语言：中文"));
        assert!(body.contains("原文语言：en"));
        assert!(build_request_body("   ", "中文", None, "m").is_none());
    }

    #[test]
    fn response_parsing_reports_provider_errors_verbatim() {
        let ok = r#"{"choices":[{"message":{"content":"用法：toybox [-h] "}}]}"#;
        assert_eq!(extract_message(ok).unwrap(), "用法：toybox [-h] ");
        let err = r#"{"error":{"message":"Invalid API key"}}"#;
        assert!(
            extract_message(err)
                .unwrap_err()
                .to_string()
                .contains("Invalid API key")
        );
        assert!(
            extract_message("<html>502</html>").is_err(),
            "非 JSON 要报错而不是当成译文"
        );
        assert!(extract_message(r#"{"choices":[]}"#).is_err());
    }

    /// 用本机一次性 HTTP 服务把**整条 curl 链路**跑通：
    /// 不是只看我把参数拼对了没，而是让 curl 真的发一次，看服务端收到的请求头/请求体，
    /// 以及临时文件有没有清干净（key 与原文都不该留在 /tmp）。
    #[tokio::test]
    async fn translate_round_trip_against_a_local_fake_endpoint() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let service = AiService::new(config.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("本机应能起一个临时监听");
        let addr = listener.local_addr().unwrap();
        // 本机明文地址要能被接受（否则这条腿自己就跑不起来），而远端明文要被拒
        service
            .save(
                &format!("http://{addr}/v1"),
                "test-model",
                true,
                Some("sk-local-test-key-9f3"),
            )
            .expect("本机 http 接口应当允许");

        let before: Vec<String> = std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("artool-ai-"))
                    .collect()
            })
            .unwrap_or_default();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("应当有一个请求");
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let length: usize = text
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|rest| rest.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = vec![0_u8; length];
            if length > 0 {
                socket.read_exact(&mut body).await.expect("读请求体");
            }
            let reply = serde_json::json!({
                "choices": [{ "message": { "content": "用法：toybox [-h] <文件>" } }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            socket.write_all(response.as_bytes()).await.ok();
            (text, String::from_utf8_lossy(&body).into_owned())
        });

        let outcome = service
            .translate("Usage: toybox [-h] <file>", "中文", Some("en"))
            .await
            .expect("本机假接口应当返回译文");
        assert_eq!(outcome.text, "用法：toybox [-h] <文件>");
        assert!(!outcome.source_truncated);
        assert!(outcome.sent_chars > 0);

        let (head, body) = server.await.expect("服务端任务应正常结束");
        assert!(
            head.contains("Authorization: Bearer sk-local-test-key-9f3"),
            "请求头必须真的带上 key（走文件而不是命令行）：{head}"
        );
        assert!(
            head.contains("Content-Type: application/json"),
            "缺 JSON 头很多网关会直接拒：{head}"
        );
        assert!(
            head.contains("POST /v1/chat/completions"),
            "路径归一：{head}"
        );
        let value: serde_json::Value = serde_json::from_str(&body).expect("请求体是 JSON");
        assert_eq!(value["model"], "test-model");
        assert!(
            value["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("Usage: toybox [-h] <file>"),
            "待译原文要原样在请求里"
        );

        let after: Vec<String> = std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("artool-ai-"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            before.len(),
            after.len(),
            "临时文件必须随调用一起删掉：{:?} -> {:?}",
            before,
            after
        );

        // key 只以后 4 位出视图；界面拿不到本体
        let view = service.view();
        assert!(view.has_api_key);
        assert_eq!(
            view.key_tail,
            "k-9f3"
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .iter()
                .rev()
                .cloned()
                .collect::<String>()
        );
        let rendered = format!("{view:?}");
        assert!(
            !rendered.contains("sk-local-test-key"),
            "配置视图里不该出现 key 本体：{rendered}"
        );
    }

    /// 关掉翻译就别去碰接口：这条判据要落在服务侧，不靠界面记得不点。
    #[tokio::test]
    async fn translate_is_refused_before_any_request_when_disabled() {
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let service = AiService::new(config);
        let error = service
            .translate("Usage", "中文", None)
            .await
            .expect_err("没配接口不该发请求");
        assert!(error.to_string().contains("翻译没有开启"), "{error}");
        service
            .save("https://api.example.com/v1", "m", true, Some("  "))
            .expect_err("开着翻译却把 key 清空是配错");
        let view = service
            .save("https://api.example.com/v1", "m", false, None)
            .unwrap();
        assert!(!view.enabled);
        assert!(
            service.translate("Usage", "中文", None).await.is_err(),
            "关掉之后仍然不该去碰接口"
        );
    }

    #[test]
    fn key_tail_only_keeps_the_last_four() {
        assert_eq!(tail("sk-abcdef1234", 4), "1234");
        assert_eq!(tail("sk-abcdef12345", 4), "2345");
        assert_eq!(tail("", 4), "");
        assert_eq!(tail("短", 4), "短");
        assert_eq!(tail("中文后缀", 2), "后缀");
    }
}
