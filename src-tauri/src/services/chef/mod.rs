//! 算法中心（CyberChef 移植）的服务层。
//!
//! CyberChef 本体是浏览器端的 JS 项目，这里把它的执行模型搬到 Rust：
//! - 一份**内置算法注册表**（`spec::REGISTRY`）取代前端的 operations.json；
//! - 一条**配方**（有序步骤 + 每步参数）= `RecipeStep` 列表，逐步把字节喂给下一步；
//! - 界面只负责「清单、拖拽组合、输入输出」，算法一律在后端跑（不引 JS 引擎、
//!   也不把 crypto 逻辑散进 WebView）。
//!
//! 为什么整体一次 IPC 而不是每步一次调用：
//! 中间步骤常常是几百 KB 的二进制。逐步 IPC 会把 Base64/Hex 之类的展示层转成
//! 字符串再传回，既慢又会在非 UTF-8 处丢字节。一次 bake 交出全部步骤的字节数与
//! 预览，界面才能显示「第 3 步开始变乱了」。

pub mod data;
pub mod ops;
pub mod spec;

use crate::models::chef::{ChefBakeRequest, ChefBakeResult, ChefOperationDto, ChefStepResult};

#[cfg(test)]
use crate::models::chef::ChefStepInput;

use data::{ChefData, ChefResult, fail};
use spec::{fill_defaults, find};

/// 输入上限：8 MiB。再大就把主线程的复制时间变成用户能感觉到的卡顿，
/// 而且多半是误粘了整个 APK。
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// 单步输出上限（解压炸弹防护：gzip 一个 1 KB 文件可以炸出 GB 级结果）
pub const MAX_STEP_BYTES: usize = 64 * 1024 * 1024;
/// 配方长度上限（CyberChef 不限，但界面每步都要渲染参数表单）
pub const MAX_STEPS: usize = 128;

/// 输入框里那段文本按什么方式变成字节
/// （`utf8` = 直接取字节；`hex` / `base64` / `decimal` = 先解一层）
pub const INPUT_ENCODINGS: &[&str] = &["utf8", "hex", "base64", "base64url", "decimal"];

pub struct ChefService;

impl ChefService {
    pub fn operations(&self) -> Vec<ChefOperationDto> {
        spec::operations()
    }

    pub fn bake(&self, request: &ChefBakeRequest) -> ChefResult<ChefBakeResult> {
        if request.recipe.len() > MAX_STEPS {
            return Err(fail(format!(
                "配方最多 {MAX_STEPS} 步，当前 {} 步",
                request.recipe.len()
            )));
        }
        let encoding = request
            .input_encoding
            .as_deref()
            .unwrap_or("utf8")
            .to_string();
        if !INPUT_ENCODINGS.contains(&encoding.as_str()) {
            return Err(fail(format!("未知的输入格式「{encoding}」")));
        }
        let seeded = seed_input(&request.input, &encoding)?;
        let input_bytes = seeded.len();
        if input_bytes > MAX_INPUT_BYTES {
            return Err(fail(format!(
                "输入 {input_bytes} 字节，超过上限 {MAX_INPUT_BYTES} 字节。把要处理的内容切一段进来",
            )));
        }

        let mut current = seeded;
        let mut steps: Vec<ChefStepResult> = Vec::with_capacity(request.recipe.len());
        let mut failed_at: Option<usize> = None;

        for (index, step) in request.recipe.iter().enumerate() {
            let entry = match find(&step.id) {
                Some(entry) => entry,
                None => {
                    let error = spec::unknown_operation(&step.id);
                    steps.push(ChefStepResult {
                        id: step.id.clone(),
                        ok: false,
                        bytes: current.len(),
                        utf8: current.is_utf8(),
                        preview: preview(&current),
                        error: Some(error.message),
                    });
                    failed_at = Some(index);
                    break;
                }
            };
            let args = fill_defaults(entry, &step.args);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (entry.run)(&current, &args)
            }));
            let record = match outcome {
                Ok(Ok(next)) => {
                    if next.len() > MAX_STEP_BYTES {
                        steps.push(ChefStepResult {
                            id: step.id.clone(),
                            ok: false,
                            bytes: next.len(),
                            utf8: false,
                            preview: String::new(),
                            error: Some(format!(
                                "第 {} 步产出 {} 字节，超过上限 {} 字节（可能是压缩炸弹）",
                                index + 1,
                                next.len(),
                                MAX_STEP_BYTES
                            )),
                        });
                        failed_at = Some(index);
                        break;
                    }
                    let record = ChefStepResult {
                        id: step.id.clone(),
                        ok: true,
                        bytes: next.len(),
                        utf8: next.is_utf8(),
                        preview: preview(&next),
                        error: None,
                    };
                    current = next;
                    record
                }
                Ok(Err(error)) => {
                    let record = ChefStepResult {
                        id: step.id.clone(),
                        ok: false,
                        bytes: current.len(),
                        utf8: current.is_utf8(),
                        preview: preview(&current),
                        error: Some(error.message),
                    };
                    steps.push(record);
                    failed_at = Some(index);
                    break;
                }
                Err(_) => {
                    // 算法实现里的 panic 不该把整条 IPC 通道打死：转成这一步的错误
                    let record = ChefStepResult {
                        id: step.id.clone(),
                        ok: false,
                        bytes: current.len(),
                        utf8: current.is_utf8(),
                        preview: preview(&current),
                        error: Some("算法内部异常（已按这一步的失败处理，请报告这个配方）".into()),
                    };
                    steps.push(record);
                    failed_at = Some(index);
                    break;
                }
            };
            steps.push(record);
        }

        let output_hex = data::hex_encode(&current.bytes, false, "");
        let output_base64 = data::b64_encode(&current.bytes, false, true);
        Ok(ChefBakeResult {
            ok: failed_at.is_none(),
            output: current.text_lossy(),
            output_bytes: current.len(),
            output_utf8: current.is_utf8(),
            output_hex,
            output_base64,
            steps,
            failed_at,
            input_bytes,
        })
    }
}

/// 把输入框的文本变成流水线起点字节
fn seed_input(text: &str, encoding: &str) -> ChefResult<ChefData> {
    match encoding {
        "utf8" => Ok(ChefData::from_text(text)),
        "hex" => data::hex_decode(text).map(ChefData::new),
        "base64" => data::b64_decode(text, false, false).map(ChefData::new),
        "base64url" => data::b64_decode(text, true, false).map(ChefData::new),
        "decimal" => data::read_by_format(&ChefData::from_text(text), "decimal").map(ChefData::new),
        other => Err(fail(format!("未知的输入格式「{other}」"))),
    }
}

/// 预览只取前 160 个字符：够定位「从这步开始变乱」，又不会把整块结果复制两份
fn preview(data: &ChefData) -> String {
    let text = data.text_lossy();
    let mut out: String = text.chars().take(160).collect();
    if text.chars().count() > 160 {
        out.push('…');
    }
    out
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::BTreeMap;

    /// 走注册表执行一个算法：默认值、id 拼写、参数名都在这条路径上被一起验掉，
    /// 所以 ops/* 的单测一律用它，而不是直接调函数（直接调会绕过默认值装配）。
    pub fn run_op(id: &str, input: &str, overrides: &[(&str, &str)]) -> ChefData {
        match try_run_op(id, input, overrides) {
            Ok(data) => data,
            Err(error) => panic!("算法 {id} 执行失败: {}", error.message),
        }
    }

    pub fn run_op_text(id: &str, input: &str, overrides: &[(&str, &str)]) -> String {
        run_op(id, input, overrides).text_lossy()
    }

    /// 输入本来就是字节的测试（GBK、加密产物、URL-safe Base64 等）必须走这条：
    /// 用 `&str` 假装二进制会被 UTF-8 重编码，测的就不是那条路径了。
    pub fn run_op_bytes(id: &str, input: &[u8], overrides: &[(&str, &str)]) -> ChefData {
        let entry = find(id).unwrap_or_else(|| panic!("注册表里没有算法 id「{id}」"));
        let args = fill_defaults(entry, &map(overrides));
        match (entry.run)(&ChefData::new(input.to_vec()), &args) {
            Ok(data) => data,
            Err(error) => panic!("算法 {id} 执行失败: {}", error.message),
        }
    }

    pub fn try_run_op(id: &str, input: &str, overrides: &[(&str, &str)]) -> ChefResult<ChefData> {
        let entry = find(id).unwrap_or_else(|| panic!("注册表里没有算法 id「{id}」"));
        let args = fill_defaults(entry, &map(overrides));
        (entry.run)(&ChefData::from_text(input), &args)
    }

    pub fn expect_error(id: &str, input: &str, overrides: &[(&str, &str)]) -> String {
        let entry = find(id).unwrap_or_else(|| panic!("注册表里没有算法 id「{id}」"));
        let args = fill_defaults(entry, &map(overrides));
        match (entry.run)(&ChefData::from_text(input), &args) {
            Ok(_) => panic!("算法 {id} 本该失败，却成功了"),
            Err(error) => error.message,
        }
    }

    fn map(overrides: &[(&str, &str)]) -> BTreeMap<String, String> {
        overrides
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// 跑一整条配方（`(算法 id, 该步参数)`）；结构性失败直接 panic
    pub fn bake(recipe: &[(&str, &[(&str, &str)])], input: &str) -> ChefBakeResult {
        ChefService
            .bake(&ChefBakeRequest {
                input: input.to_string(),
                input_encoding: Some("utf8".into()),
                recipe: recipe
                    .iter()
                    .map(|(id, args)| ChefStepInput {
                        id: (*id).to_string(),
                        args: map(args),
                    })
                    .collect(),
            })
            .unwrap_or_else(|e| panic!("{}", e.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn registry_ids_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for entry in spec::REGISTRY.iter() {
            assert!(!seen.contains(&entry.id), "算法 id 重复：{}", entry.id);
            seen.push(entry.id);
            assert!(!entry.category.is_empty(), "算法 {} 没有分类", entry.id);
            let mut arg_names: Vec<&str> = Vec::new();
            for arg in entry.args {
                assert!(
                    !arg_names.contains(&arg.name),
                    "算法 {} 参数名重复：{}",
                    entry.id,
                    arg.name
                );
                arg_names.push(arg.name);
            }
        }
        assert!(
            seen.len() >= 60,
            "算法数量掉到 {}，注册表是不是漏接了一组",
            seen.len()
        );
    }

    #[test]
    fn every_operation_runs_with_registry_defaults() {
        // 冒烟：每个算法都用注册表默认参数跑一遍「abc」。允许业务性失败
        // （需要密钥的加密本来就该拒绝），但不允许空消息或 panic。
        for entry in spec::REGISTRY.iter() {
            let outcome = testing::try_run_op(
                entry.id,
                "abc",
                &[
                    ("key", "00112233445566778899aabbccddeeff"),
                    ("iv", "00112233445566778899aabbccddeeff"),
                    ("nonce", "00112233445566778899"),
                    ("privateKey", "x"),
                    ("publicKey", "x"),
                    ("search", "a"),
                    ("pattern", "a"),
                    ("hash", "abc"),
                    ("secret", "k"),
                    ("salt", "s"),
                    ("password", "p"),
                ],
            );
            if let Err(error) = outcome {
                assert!(
                    !error.message.is_empty(),
                    "算法 {} 的失败消息为空",
                    entry.id
                );
            }
        }
    }

    #[test]
    fn pipeline_feeds_bytes_not_strings() {
        // gzip 的产物是二进制，必须能直接接 Base64：中间步骤按字节传是这套设计的关键
        let result = testing::bake(&[("gzip", &[]), ("base64Encode", &[])], "hello");
        assert!(result.ok, "{:?}", result.steps);
        assert!(result.output_utf8, "Base64 输出应当是文本");
        assert!(result.output.starts_with("H4sI"), "{}", result.output);
        assert_eq!(result.steps.len(), 2);
        assert!(result.steps[0].bytes > 15, "gzip 之后是二进制");
        assert!(!result.steps[0].utf8, "gzip 输出不该被当成文本");
    }

    #[test]
    fn unknown_step_id_fails_that_step_only_and_stops_pipeline() {
        let result = testing::bake(
            &[
                ("base64Encode", &[]),
                ("noSuchOp", &[]),
                ("base64Decode", &[]),
            ],
            "hi",
        );
        assert!(!result.ok);
        assert_eq!(result.failed_at, Some(1));
        assert_eq!(result.steps.len(), 2, "失败之后的步骤不该再跑");
        assert!(
            result.steps[1]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("noSuchOp")
        );
    }

    #[test]
    fn step_error_is_reported_with_its_index() {
        let result = testing::bake(&[("base64Decode", &[]), ("md5", &[])], "!!!!");
        assert_eq!(result.failed_at, Some(0));
        assert!(result.steps[0].error.as_ref().unwrap().contains("Base64"));
    }

    #[test]
    fn step_args_are_isolated() {
        // 第一步不换行、第二步严格解码：参数不能串到别的步骤上
        let result = testing::bake(
            &[
                ("base64Encode", &[("noWrap", "true")]),
                ("base64Decode", &[("strict", "true")]),
            ],
            "hello",
        );
        assert!(result.ok, "{:?}", result.steps);
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn input_encoding_selects_how_the_textarea_becomes_bytes() {
        let request = ChefBakeRequest {
            input: "616263".to_string(),
            input_encoding: Some("hex".to_string()),
            recipe: vec![],
        };
        let result = ChefService.bake(&request).unwrap();
        assert_eq!(result.output, "abc");
        assert_eq!(result.input_bytes, 3);

        let bad = ChefBakeRequest {
            input: "zz".to_string(),
            input_encoding: Some("hex".to_string()),
            recipe: vec![],
        };
        let error = ChefService.bake(&bad).unwrap_err();
        assert!(error.message.contains("十六进制"), "{}", error.message);

        let unknown = ChefBakeRequest {
            input: "x".to_string(),
            input_encoding: Some("utf16".to_string()),
            recipe: vec![],
        };
        let error = ChefService.bake(&unknown).unwrap_err();
        assert!(
            error.message.contains("未知的输入格式"),
            "{}",
            error.message
        );
    }

    #[test]
    fn oversized_input_is_rejected_before_running() {
        let huge = "x".repeat(MAX_INPUT_BYTES + 8);
        let request = ChefBakeRequest {
            input: huge,
            input_encoding: None,
            recipe: vec![],
        };
        let error = ChefService.bake(&request).unwrap_err();
        assert!(error.message.contains("上限"), "{}", error.message);
    }

    #[test]
    fn oversized_step_output_is_stopped() {
        // 解压炸弹口径：某一步膨胀过上限时报错并停在那里，而不是把内存吃干
        let mut args = BTreeMap::new();
        args.insert("count".to_string(), "99999999".to_string());
        let request = ChefBakeRequest {
            input: "a".to_string(),
            input_encoding: Some("utf8".to_string()),
            recipe: vec![ChefStepInput {
                id: "repeatString".into(),
                args,
            }],
        };
        let result = ChefService.bake(&request).unwrap();
        assert!(!result.ok, "重复出来的超大结果必须被上限拦住");
        assert_eq!(result.failed_at, Some(0));
        // 具体是哪道闸拦的都行：算法自己的上限（重复次数）或服务的单步上限，措辞都带「上限」
        let message = result.steps[0].error.as_ref().unwrap().clone();
        assert!(message.contains("上限"), "{message}");
        assert!(
            message.contains("压缩炸弹") || message.contains("重复"),
            "{message}"
        );
    }

    #[test]
    fn dto_shape_is_camel_case_and_carries_defaults() {
        let ops = ChefService.operations();
        let aes = ops
            .iter()
            .find(|o| o.id == "aesEncrypt")
            .expect("有 aesEncrypt");
        let json = serde_json::to_string(aes).unwrap();
        assert!(json.contains(r#""inputFormat""#), "{json}");
        assert!(json.contains(r#""default""#), "{json}");
        assert!(json.contains(r#""options""#), "{json}");
    }

    #[test]
    fn recipe_deserializes_from_frontend_json() {
        let raw = serde_json::json!({
            "input": "hello",
            "inputEncoding": "utf8",
            "recipe": [
                { "id": "base64Encode", "args": { "noWrap": "true" } },
                { "id": "md5", "args": {} }
            ]
        });
        let request: ChefBakeRequest = serde_json::from_value(raw).unwrap();
        let result = ChefService.bake(&request).unwrap();
        assert_eq!(result.output, "0733351879b2fa9bd05c7ca3061529c0");
    }

    #[test]
    fn defaults_come_from_the_registry() {
        let entry = spec::find("hexEncode").unwrap();
        let filled = fill_defaults(entry, &BTreeMap::new());
        assert_eq!(filled.text("separator", "!!"), "");
        assert!(!filled.bool_of("uppercase"));
        // 显式清空与「没填」是两回事：空串要原样传下去
        let mut cleared = BTreeMap::new();
        cleared.insert("separator".to_string(), " ".to_string());
        assert_eq!(fill_defaults(entry, &cleared).text("separator", "!!"), " ");
    }
}
