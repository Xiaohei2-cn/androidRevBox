//! 算法中心的数据模型：流水线里流动的**字节**、算法参数、以及一批编解码小工具。
//!
//! 为什么一律按字节流动（CyberChef 同模型）：
//! 脱壳/逆向时最常见的是「压缩 → 加密 → Base64」这类混合链，中间步骤产出的是
//! 任意二进制。若按字符串传递，第一步的非 UTF-8 字节就被毁了，后面全错。
//! 所以：文本类算法在读入时**严格要求 UTF-8**，写出时按 UTF-8 编码；
//! 其余算法只在字节层面工作。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 算法失败的错误（消息为中文可读上下文，前端原样展示在出错的那一步上）。
#[derive(Debug, Clone)]
pub struct ChefError {
    pub message: String,
}

impl ChefError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ChefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ChefResult<T> = Result<T, ChefError>;

/// 把任何错误压成算法错误（算法内部一律用 `?` + 这个转换，避免每种 crate 各写一遍）
pub fn fail(message: impl Into<String>) -> ChefError {
    ChefError::new(message)
}

/// 流水线数据体
#[derive(Debug, Clone)]
pub struct ChefData {
    pub bytes: Vec<u8>,
}

impl ChefData {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn from_text(text: &str) -> Self {
        Self::new(text.as_bytes().to_vec())
    }

    /// 严格要求 UTF-8：文本算法读入时用它，坏字节直接报错而不是悄悄替换成 U+FFFD
    /// （悄悄替换会让「看起来成功了但结果不对」这种最难查的故障进用户视野）
    pub fn text(&self) -> ChefResult<&str> {
        std::str::from_utf8(&self.bytes).map_err(|_| {
            fail("当前数据不是合法 UTF-8 文本，这一步需要文本输入（先用上一步把它转成文本）")
        })
    }

    /// 展示用：非 UTF-8 时按 lossy 渲染（输出框里出现替换符比整块空白有用）
    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn is_utf8(&self) -> bool {
        std::str::from_utf8(&self.bytes).is_ok()
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// 算法参数：前端表单一律交字符串（复选框交 "true"/"false"，数字交十进制文本）。
/// 用 BTreeMap 而不是 HashMap：同一份配方每次都按同样顺序遍历，
/// 报错消息与测试断言才是可复现的。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Args(BTreeMap<String, String>);

impl From<BTreeMap<String, String>> for Args {
    fn from(map: BTreeMap<String, String>) -> Self {
        Self(map)
    }
}

impl Args {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn text(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or(default).to_string()
    }

    /// 复选框：只认 "true"（其他值算 false）。空串/缺键一律 false。
    pub fn bool_of(&self, key: &str) -> bool {
        self.get(key) == Some("true")
    }

    pub fn number(&self, key: &str, default: u64) -> ChefResult<u64> {
        match self.get(key) {
            None | Some("") => Ok(default),
            Some(raw) => raw
                .trim()
                .strip_prefix("0x")
                .and_then(|h| u64::from_str_radix(h, 16).ok())
                .or_else(|| raw.trim().parse::<u64>().ok())
                .ok_or_else(|| fail(format!("参数 {key} 需要是数字，收到「{raw}」"))),
        }
    }

    /// 下拉框取值（缺键回默认）
    pub fn choice(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or(default).to_string()
    }

    /// 下拉框取值并校验在允许集合内：把前端传歪的 option 变成明确报错，
    /// 而不是悄悄走最后一个分支给出看似成功的结果
    pub fn choice_in(&self, key: &str, default: &str, options: &[&str]) -> ChefResult<String> {
        let value = self.choice(key, default);
        if options.contains(&value.as_str()) {
            Ok(value)
        } else {
            Err(fail(format!(
                "参数 {key} 只接受 {}，收到「{value}」",
                options.join(" / ")
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// 十六进制
// ---------------------------------------------------------------------------

pub fn hex_encode(bytes: &[u8], upper: bool, separator: &str) -> String {
    let mapped: Vec<String> = bytes
        .iter()
        .map(|b| {
            let s = format!("{b:02x}");
            if upper { s.to_uppercase() } else { s }
        })
        .collect();
    mapped.join(separator)
}

/// 解析十六进制文本：允许空白、允许 0x 前缀、允许奇数长度（左侧补 0，
/// 与 CyberChef 的「From Hex」容错一致：`f00b` → `0xf0 0x0b`）
pub fn hex_decode(input: &str) -> ChefResult<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let cleaned = cleaned
        .strip_prefix("0x")
        .unwrap_or(cleaned.as_str())
        .to_string();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        let bad = cleaned
            .chars()
            .find(|c| !c.is_ascii_hexdigit())
            .map(|c| c.to_string())
            .unwrap_or_default();
        return Err(fail(format!("十六进制里出现了非法字符「{bad}」")));
    }
    let padded = if cleaned.len() % 2 == 1 {
        format!("0{cleaned}")
    } else {
        cleaned
    };
    let mut out = Vec::with_capacity(padded.len() / 2);
    let bytes = padded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16).expect("已校验为 hex");
        let lo = (bytes[i + 1] as char).to_digit(16).expect("已校验为 hex");
        out.push((hi << 4 | lo) as u8);
        i += 2;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Base64 / Base32 / Base85 / Base91
// ---------------------------------------------------------------------------

use base64::Engine;

/// Base64 输出的 MIME 换行宽度（76 字符/行，PEM 与证书里就是这个口径）
const B64_WRAP_WIDTH: usize = 76;

pub fn b64_encode(bytes: &[u8], url_safe: bool, no_wrap: bool) -> String {
    // 与 CyberChef 一致：默认**保留** `=` 填充；noWrap 只管要不要每 76 字符断行
    let engine = if url_safe {
        base64::engine::general_purpose::URL_SAFE
    } else {
        base64::engine::general_purpose::STANDARD
    };
    let body = engine.encode(bytes);
    if no_wrap { body } else { wrap_b64(&body) }
}

fn wrap_b64(body: &str) -> String {
    body.as_bytes()
        .chunks(B64_WRAP_WIDTH)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Base64 解码。`strict=true` 时要求补齐 `=`；宽松模式先按原样试，
/// 再去掉空白/换行、把 `-`/`_` 当 `+`/`/`、最后按长度补 padding。
pub fn b64_decode(input: &str, url_safe: bool, strict: bool) -> ChefResult<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let engine = if strict {
        if url_safe {
            base64::engine::general_purpose::URL_SAFE
        } else {
            base64::engine::general_purpose::STANDARD
        }
    } else if url_safe {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
    } else {
        base64::engine::general_purpose::STANDARD_NO_PAD
    };
    let body = if strict {
        cleaned.clone()
    } else {
        // 宽松模式：把另一种字母表的写法折算到**当前引擎**这一侧（-/_ 与 +// 互兜），
        // 再剥掉可给可不给的 padding。方向搞反就会把合法的 '+' 报成非法字符。
        cleaned
            .chars()
            .map(|c| {
                if url_safe {
                    match c {
                        '+' => '-',
                        '/' => '_',
                        other => other,
                    }
                } else {
                    match c {
                        '-' => '+',
                        '_' => '/',
                        other => other,
                    }
                }
            })
            .collect::<String>()
            .trim_end_matches('=')
            .to_string()
    };
    engine.decode(body).map_err(|e| {
        fail(format!(
            "Base64 解码失败：{e}（非法字符、缺 padding 或长度不对）"
        ))
    })
}

/// 任意进制（Base58/Base62 这类「大整数按进制展开」的编码）
pub fn radix_encode(bytes: &[u8], alphabet: &[u8]) -> String {
    let zero = alphabet[0] as char;
    let mut leading = 0usize;
    for b in bytes {
        if *b == 0 {
            leading += 1;
        } else {
            break;
        }
    }
    let mut digits: Vec<u8> = Vec::new();
    let mut number: Vec<u8> = bytes[leading..].to_vec();
    let radix = alphabet.len() as u64;
    while !number.is_empty() {
        let mut remainder = 0u64;
        let mut next = Vec::with_capacity(number.len());
        for byte in &number {
            let acc = remainder * 256 + *byte as u64;
            let q = acc / radix;
            remainder = acc % radix;
            if !next.is_empty() || q != 0 {
                next.push(q as u8);
            }
        }
        digits.push(alphabet[remainder as usize]);
        number = next;
    }
    let mut out = String::new();
    for _ in 0..leading {
        out.push(zero);
    }
    for d in digits.iter().rev() {
        out.push(*d as char);
    }
    out
}

pub fn radix_decode(input: &str, alphabet: &[u8]) -> ChefResult<Vec<u8>> {
    let zero = alphabet[0] as char;
    let mut leading = 0usize;
    for c in input.chars() {
        if c == zero {
            leading += 1;
        } else {
            break;
        }
    }
    let radix = alphabet.len() as u64;
    let mut bytes: Vec<u8> = Vec::new();
    for c in input.chars().skip(leading) {
        let value = alphabet
            .iter()
            .position(|a| *a as char == c)
            .ok_or_else(|| fail(format!("字母表里没有字符「{c}」，这不是该进制的编码")))?;
        let mut carry = value as u64;
        for slot in bytes.iter_mut().rev() {
            let acc = *slot as u64 * radix + carry;
            *slot = acc as u8;
            carry = acc >> 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    let mut out = vec![0u8; leading];
    out.extend(bytes);
    Ok(out)
}

/// Base85 家族的两套字母表：
/// - `adobe`：Ascii85（字符 33..117，`z` 代表四个零字节，可选 `<~ ~>`` 定界）
/// - `base85`/`z85`：RFC 1924 字母表（Z85 用它，不带缩写与定界）
fn base85_alphabet(style: &str) -> ChefResult<(&'static [u8], bool)> {
    match style {
        "adobe" => Ok((
            b"!\"#$%&\'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}",
            true,
        )),
        "base85" | "z85" => Ok((
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~",
            false,
        )),
        other => Err(fail(format!("未知的 Base85 风格「{other}」"))),
    }
}

pub const BASE85_STYLES: &[&str] = &["adobe", "base85", "z85"];

pub fn a85_encode(bytes: &[u8], style: &str, delimiters: bool) -> ChefResult<String> {
    let (alphabet, shortcut) = base85_alphabet(style)?;
    let mut out = String::new();
    if delimiters && style == "adobe" {
        out.push_str("<~");
    }
    for chunk in bytes.chunks(4) {
        if chunk.len() == 4 && chunk.iter().all(|b| *b == 0) && shortcut {
            out.push('z');
            continue;
        }
        let pad = 4 - chunk.len();
        let mut value: u32 = 0;
        for b in chunk {
            value = (value << 8) | *b as u32;
        }
        value <<= pad * 8;
        let mut digits = [0u8; 5];
        let mut rest = value;
        for slot in digits.iter_mut().rev() {
            *slot = (rest % 85) as u8;
            rest /= 85;
        }
        // 不满 4 字节的尾组只输出「字节数 + 1」个字符（两套字母表同规则）
        for k in 0..(5 - pad) {
            out.push(alphabet[digits[k] as usize] as char);
        }
    }
    if delimiters && style == "adobe" {
        out.push_str("~>");
    }
    Ok(out)
}

pub fn a85_decode(input: &str, style: &str) -> ChefResult<Vec<u8>> {
    let (alphabet, shortcut) = base85_alphabet(style)?;
    let max_char = alphabet[84];
    let mut body: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if style == "adobe" {
        body = body
            .trim_start_matches("<~")
            .trim_end_matches("~>")
            .to_string();
    } else if style == "z85" && body.len() % 5 != 0 {
        // Z85（libzmq）要求整组；Base85（RFC 1924）允许尾组不满 5 个字符
        return Err(fail("Z85 的长度必须是 5 的倍数（它按 5 字符一组对齐）"));
    }
    let chars: Vec<char> = body.chars().collect();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if shortcut && chars[i] == 'z' {
            out.extend([0u8; 4]);
            i += 1;
            continue;
        }
        let group = &chars[i..(i + 5).min(chars.len())];
        if group.len() < 2 {
            return Err(fail("Base85 末尾剩一个字符，无法还原成字节"));
        }
        // 尾组用字母表里最大的字符补齐，再丢掉补出来的那几个字节
        let mut value: u32 = 0;
        for k in 0..5 {
            let digit = match group.get(k) {
                Some(c) => alphabet
                    .iter()
                    .position(|b| *b as char == *c)
                    .ok_or_else(|| fail(format!("Base85 里出现了非法字符「{c}」")))?,
                None => 84,
            };
            value = value * 85 + digit as u32;
        }
        let _ = max_char;
        for k in 0..(group.len() - 1) {
            out.push((value >> (24 - 8 * k)) as u8);
        }
        i += 5;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 「按格式读入 / 按格式写出」——所有加解密算法共用
// ---------------------------------------------------------------------------

/// 输入侧：把当前流水线数据按用户声明的格式解释成字节
pub fn read_by_format(data: &ChefData, format: &str) -> ChefResult<Vec<u8>> {
    let text = data.text()?;
    match format {
        "utf8" | "raw" => Ok(data.bytes.clone()),
        "hex" => hex_decode(text),
        "base64" => b64_decode(text, false, false),
        "base64url" => b64_decode(text, true, false),
        "decimal" => Ok(text
            .split([' ', ',', '\n', '\t'])
            .filter(|t| !t.is_empty())
            .map(|t| {
                t.trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|v| *v < 256)
                    .map(|v| v as u8)
                    .ok_or_else(|| fail(format!("十进制字节列表里有超过 255 或非法的「{t}」")))
            })
            .collect::<ChefResult<Vec<u8>>>()?),
        other => Err(fail(format!("未知的输入格式「{other}」"))),
    }
}

/// 输入侧（字符串参数版）：密钥、IV 这类由用户直接填的参数
pub fn parse_param_bytes(text: &str, format: &str) -> ChefResult<Vec<u8>> {
    match format {
        "utf8" | "raw" => Ok(text.as_bytes().to_vec()),
        "hex" => hex_decode(text),
        "base64" => b64_decode(text, false, false),
        "base64url" => b64_decode(text, true, false),
        other => Err(fail(format!("未知的参数格式「{other}」"))),
    }
}

/// 输出侧：把结果字节按用户选的格式渲染成流水线数据（下游继续按字节走）
pub fn emit_by_format(bytes: Vec<u8>, format: &str) -> ChefResult<ChefData> {
    match format {
        "utf8" | "raw" | "binary" => Ok(ChefData::new(bytes)),
        "hex" => Ok(ChefData::from_text(&hex_encode(&bytes, false, ""))),
        "base64" => Ok(ChefData::from_text(&b64_encode(&bytes, false, true))),
        "base64url" => Ok(ChefData::from_text(&b64_encode(&bytes, true, true))),
        "decimal" => {
            let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
            Ok(ChefData::from_text(&list.join(" ")))
        }
        other => Err(fail(format!("未知的输出格式「{other}」"))),
    }
}

/// 校验长度并把越界情况说成人话（密钥长度错是加密里最高频的失败）
pub fn expect_len(bytes: &[u8], len: usize, what: &str) -> ChefResult<()> {
    if bytes.len() == len {
        Ok(())
    } else {
        Err(fail(format!(
            "{what}长度应为 {len} 字节，实际 {} 字节（检查密钥/IV 的格式选对没有）",
            bytes.len()
        )))
    }
}
