//! 编码类算法：Base64/Base32/Base58/Base62/Base85、十六进制、数字列表、URL、
//! HTML 实体、Unicode 转义、字符集转码（GBK 等）、莫尔斯电码。
//!
//! 字符集转码是安卓逆向里最容易踩的一类：老 SDK 用 GBK 写日志、老 H5 用 GB2312 提交表单，
//! 前端 JS 的 TextDecoder 不支持时会得到一堆乱码，所以这里直接给出 encoding_rs 的全套。

use crate::services::chef::data::{
    Args, BASE85_STYLES, ChefData, ChefResult, a85_decode, a85_encode, b64_decode, b64_encode,
    fail, hex_decode, hex_encode, radix_decode, radix_encode,
};
use crate::services::chef::spec::{bool_arg, choice, op, text};

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

pub fn base64_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let url_safe = args.bool_of("urlSafe");
    let no_wrap = args.bool_of("noWrap");
    Ok(ChefData::from_text(&b64_encode(
        &data.bytes,
        url_safe,
        no_wrap,
    )))
}

pub fn base64_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let url_safe = args.bool_of("urlSafe");
    let strict = args.bool_of("strict");
    Ok(ChefData::new(b64_decode(data.text()?, url_safe, strict)?))
}

// ---------------------------------------------------------------------------
// Base32（RFC 4648）
// ---------------------------------------------------------------------------

/// data-encoding 的 RFC 4648 表位：标准/十六进制字母表 × 带/不带填充
/// data-encoding 的 BASE32/BASE32HEX 是**带填充**的那一档，NOPAD 才是无填充档
fn base32_engine(hex_alphabet: bool, padding: bool) -> data_encoding::Encoding {
    use data_encoding::BASE32;
    use data_encoding::BASE32_NOPAD;
    use data_encoding::BASE32HEX;
    use data_encoding::BASE32HEX_NOPAD;
    match (hex_alphabet, padding) {
        (false, true) => BASE32,
        (false, false) => BASE32_NOPAD,
        (true, true) => BASE32HEX,
        (true, false) => BASE32HEX_NOPAD,
    }
}

pub fn base32_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let engine = base32_engine(args.bool_of("hexAlphabet"), args.bool_of("pad"));
    Ok(ChefData::from_text(&engine.encode(&data.bytes)))
}

pub fn base32_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let hex_alphabet = args.bool_of("hexAlphabet");
    let input = data.text()?.to_uppercase();
    // 解码一律按「无填充」档走：填没给等号都能吃，给了等号就先剥掉
    let body = input.trim_end_matches('=');
    let engine = base32_engine(hex_alphabet, false);
    engine
        .decode(body.as_bytes())
        .map(ChefData::new)
        .map_err(|e| {
            fail(format!(
                "Base32 解码失败：{e}（长度不对或含字母表外的字符）"
            ))
        })
}

// ---------------------------------------------------------------------------
// Base58 / Base62（大整数进制）
// ---------------------------------------------------------------------------

const BASE58_ALPHABETS: &[(&str, &[u8])] = &[
    (
        "bitcoin",
        b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz",
    ),
    (
        "ripple",
        b"rpshnaf39b2UD567GHJTLKOMICjaEVWQBkclMXY89gzxqesAtuivwdsSnoyfFNPZ",
    ),
    (
        "flickr",
        b"123456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ",
    ),
];

const BASE62_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// 大整数进制转换是 O(n²) 的，给输入上限；否则 1 MB 数据会把 UI 拖死
const RADIX_MAX_INPUT: usize = 64 * 1024;

fn radix_guard(data: &ChefData, what: &str) -> ChefResult<()> {
    if data.len() > RADIX_MAX_INPUT {
        return Err(fail(format!(
            "{what} 是逐位大整数运算，输入上限 {RADIX_MAX_INPUT} 字节（当前 {} 字节）。先切片或换 Base64",
            data.len()
        )));
    }
    Ok(())
}

pub fn base58_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    radix_guard(data, "Base58 编码")?;
    let name = args.choice("alphabet", "bitcoin");
    let alphabet = BASE58_ALPHABETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| fail(format!("未知的 Base58 字母表「{name}」")))?;
    Ok(ChefData::from_text(&radix_encode(&data.bytes, alphabet)))
}

pub fn base58_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let name = args.choice("alphabet", "bitcoin");
    let alphabet = BASE58_ALPHABETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| fail(format!("未知的 Base58 字母表「{name}」")))?;
    radix_guard(data, "Base58 解码")?;
    Ok(ChefData::new(radix_decode(data.text()?, alphabet)?))
}

pub fn base62_encode(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    radix_guard(data, "Base62 编码")?;
    Ok(ChefData::from_text(&radix_encode(
        &data.bytes,
        BASE62_ALPHABET,
    )))
}

pub fn base62_decode(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    radix_guard(data, "Base62 解码")?;
    Ok(ChefData::new(radix_decode(data.text()?, BASE62_ALPHABET)?))
}

// ---------------------------------------------------------------------------
// Base85 / Z85
// ---------------------------------------------------------------------------

pub fn base85_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let style = args.choice_in("style", "adobe", BASE85_STYLES)?;
    let delimiters = args.bool_of("delimiters");
    Ok(ChefData::from_text(&a85_encode(
        &data.bytes,
        &style,
        delimiters,
    )?))
}

pub fn base85_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let style = args.choice_in("style", "adobe", BASE85_STYLES)?;
    Ok(ChefData::new(a85_decode(data.text()?, &style)?))
}

// ---------------------------------------------------------------------------
// 十六进制 / 数字列表（十进制、八进制、二进制）
// ---------------------------------------------------------------------------

pub fn hex_encode_op(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let upper = args.bool_of("uppercase");
    let separator = args.text("separator", "");
    Ok(ChefData::from_text(&hex_encode(
        &data.bytes,
        upper,
        &separator,
    )))
}

pub fn hex_decode_op(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::new(hex_decode(data.text()?)?))
}

/// 一个字节 → 一个进制的数字串（CyberChef 的 To Decimal / To Binary / To Octal）
fn to_radix(data: &ChefData, radix: u32, separator: &str) -> ChefData {
    let parts: Vec<String> = data
        .bytes
        .iter()
        .map(|b| match radix {
            2 => format!("{b:08b}"),
            8 => format!("{b:03o}"),
            16 => format!("{b:02x}"),
            _ => b.to_string(),
        })
        .collect();
    ChefData::from_text(&parts.join(separator))
}

fn from_radix(data: &ChefData, radix: u32, separator: &str) -> ChefResult<ChefData> {
    let text = data.text()?;
    let tokens: Vec<&str> = if separator == "\\s" || separator.is_empty() {
        text.split_whitespace().collect()
    } else {
        text.split(separator).collect()
    };
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = u32::from_str_radix(trimmed, radix).map_err(|e| {
            fail(format!(
                "第 {} 个「{trimmed}」不是 {radix} 进制字节值：{e}",
                out.len() + 1
            ))
        })?;
        if value > 255 {
            return Err(fail(format!(
                "「{trimmed}」= {value} 超出一个字节（0–255）"
            )));
        }
        out.push(value as u8);
    }
    Ok(ChefData::new(out))
}

pub fn decimal_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    Ok(to_radix(data, 10, &args.text("separator", " ")))
}

pub fn decimal_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    from_radix(data, 10, &args.text("separator", " "))
}

pub fn octal_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    Ok(to_radix(data, 8, &args.text("separator", " ")))
}

pub fn octal_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    from_radix(data, 8, &args.text("separator", " "))
}

pub fn binary_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    Ok(to_radix(data, 2, &args.text("separator", " ")))
}

pub fn binary_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    from_radix(data, 2, &args.text("separator", " "))
}

// ---------------------------------------------------------------------------
// URL 百分号编码
// ---------------------------------------------------------------------------

/// RFC 3986 未保留字符
fn is_unreserved(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')
}

/// encodeURIComponent 额外保留的字符
fn is_js_unreserved(c: char) -> bool {
    is_unreserved(c) || matches!(c, '!' | '\'' | '(' | ')' | '*' | '-')
}

pub fn url_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let component = args.bool_of("component");
    let uppercase = args.bool_of("uppercase");
    let mut out = String::new();
    for ch in data.text()?.chars() {
        let keep = if component {
            is_js_unreserved(ch)
        } else {
            is_unreserved(ch)
        };
        if keep {
            out.push(ch);
        } else {
            for byte in ch.to_string().as_bytes() {
                out.push('%');
                let encoded = if uppercase {
                    format!("{byte:02X}")
                } else {
                    format!("{byte:02x}")
                };
                out.push_str(&encoded);
            }
        }
    }
    Ok(ChefData::from_text(&out))
}

pub fn url_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let plus_as_space = args.bool_of("plusAsSpace");
    let input = data.text()?;
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() || i + 1 < bytes.len() => {
                let hi = *bytes
                    .get(i + 1)
                    .ok_or_else(|| fail("URL 编码在行尾被截断"))?;
                let lo = *bytes
                    .get(i + 2)
                    .ok_or_else(|| fail("URL 编码在行尾被截断"))?;
                let value = (hi as char)
                    .to_digit(16)
                    .and_then(|h| (lo as char).to_digit(16).map(|l| (h << 4 | l) as u8));
                match value {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    if std::str::from_utf8(&out).is_err() {
        return Err(fail(
            "解码后的字节不是合法 UTF-8：这可能是 GBK/Big5 之类编码，用「按字符集解码」再试",
        ));
    }
    Ok(ChefData::new(out))
}

// ---------------------------------------------------------------------------
// HTML 实体
// ---------------------------------------------------------------------------

const NAMED_ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{00a0}"),
    ("copy", "©"),
    ("reg", "®"),
    ("trade", "™"),
    ("hellip", "…"),
    ("mdash", "—"),
    ("ndash", "–"),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("times", "×"),
    ("divide", "÷"),
    ("middot", "·"),
    ("bull", "•"),
    ("deg", "°"),
    ("plusmn", "±"),
    ("frac12", "½"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("euro", "€"),
    ("pound", "£"),
    ("yen", "¥"),
    ("sect", "§"),
    ("para", "¶"),
    ("dagger", "†"),
    ("permil", "‰"),
    ("larr", "←"),
    ("rarr", "→"),
    ("uarr", "↑"),
    ("darr", "↓"),
    ("infin", "∞"),
    ("ne", "≠"),
    ("le", "≤"),
    ("ge", "≥"),
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("pi", "π"),
    ("sigma", "σ"),
    ("omega", "ω"),
    ("mu", "μ"),
    ("lambda", "λ"),
];

pub fn html_entity_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let style = args.choice("style", "named");
    let mut out = String::new();
    for ch in data.text()?.chars() {
        let named = match ch {
            '&' => Some("amp"),
            '<' => Some("lt"),
            '>' => Some("gt"),
            '"' => Some("quot"),
            '\'' => Some("apos"),
            _ => None,
        };
        match style.as_str() {
            "named" => {
                if let Some(entity) = named {
                    out.push('&');
                    out.push_str(entity);
                    out.push(';');
                } else if !ch.is_ascii() {
                    out.push_str(&format!("&#{};", ch as u32));
                } else {
                    out.push(ch);
                }
            }
            "decimal" => {
                if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '\n' | '\t') {
                    out.push(ch);
                } else {
                    out.push_str(&format!("&#{};", ch as u32));
                }
            }
            "hex" => {
                if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '\n' | '\t') {
                    out.push(ch);
                } else {
                    out.push_str(&format!("&#x{:x};", ch as u32));
                }
            }
            "all-non-ascii" => {
                if let Some(entity) = named {
                    out.push('&');
                    out.push_str(entity);
                    out.push(';');
                } else if ch.is_ascii() {
                    out.push(ch);
                } else {
                    out.push_str(&format!("&#{};", ch as u32));
                }
            }
            other => return Err(fail(format!("未知的实体风格「{other}」"))),
        }
    }
    Ok(ChefData::from_text(&out))
}

pub fn html_entity_decode(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(&decode_entities(data.text()?)))
}

/// 未识别的实体（词典里没有的 `&xxx;`）原样保留：解码算法不能吃掉看不懂的字节
fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(end) = tail.find(';') {
            if let Some(text) = decode_entity(&tail[1..end]) {
                out.push_str(&text);
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    if let Some(hex) = entity.strip_prefix("#x").or(entity.strip_prefix("#X")) {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    if let Some(dec) = entity.strip_prefix('#') {
        return dec
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    NAMED_ENTITIES
        .iter()
        .find(|(name, _)| *name == entity)
        .map(|(_, value)| value.to_string())
}

// ---------------------------------------------------------------------------
// Unicode 转义
// ---------------------------------------------------------------------------

pub fn unicode_escape(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let non_ascii_only = args.bool_of("nonAsciiOnly");
    let mut out = String::new();
    for ch in data.text()?.chars() {
        let needs_escape = !ch.is_ascii() || ch == '\\' || (ch as u32) < 0x20;
        if !non_ascii_only || needs_escape {
            let cp = ch as u32;
            if cp > 0xFFFF {
                // 代理对：Java/C 源码里常见写法
                let v = cp - 0x1_0000;
                let high = 0xD800 + (v >> 10);
                let low = 0xDC00 + (v & 0x3FF);
                out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            } else {
                out.push_str(&format!("\\u{cp:04x}"));
            }
        } else {
            out.push(ch);
        }
    }
    Ok(ChefData::from_text(&out))
}

pub fn unicode_unescape(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let input = data.text()?;
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                'u' | 'x' => {
                    let width = if chars[i + 1] == 'u' { 4 } else { 2 };
                    let digits: String = chars
                        .iter()
                        .skip(i + 2)
                        .take(width)
                        .filter(|c| c.is_ascii_hexdigit())
                        .collect();
                    if digits.len() == width {
                        let value = u32::from_str_radix(&digits, 16).expect("已校验 hex");
                        // 处理 Java 风格的代理对
                        if (0xD800..0xDC00).contains(&value)
                            && i + 2 + width + 2 + width <= chars.len()
                            && chars[i + 2 + width] == '\\'
                            && chars[i + 3 + width] == 'u'
                        {
                            let low_digits: String = chars
                                .iter()
                                .skip(i + 4 + width)
                                .take(width)
                                .filter(|c| c.is_ascii_hexdigit())
                                .collect();
                            if let Ok(low) = u32::from_str_radix(&low_digits, 16) {
                                if (0xDC00..0xE000).contains(&low) {
                                    let cp = 0x1_0000 + ((value - 0xD800) << 10) + (low - 0xDC00);
                                    if let Some(c) = char::from_u32(cp) {
                                        out.push(c);
                                        i += 4 + width * 2;
                                        continue;
                                    }
                                }
                            }
                        }
                        if let Some(c) = char::from_u32(value) {
                            out.push(c);
                            i += 2 + width;
                            continue;
                        }
                    }
                    out.push(chars[i]);
                    i += 1;
                }
                'n' => {
                    out.push('\n');
                    i += 2;
                }
                'r' => {
                    out.push('\r');
                    i += 2;
                }
                't' => {
                    out.push('\t');
                    i += 2;
                }
                '0' => {
                    out.push('\0');
                    i += 2;
                }
                '\\' => {
                    out.push('\\');
                    i += 2;
                }
                '"' => {
                    out.push('"');
                    i += 2;
                }
                '\'' => {
                    out.push('\'');
                    i += 2;
                }
                other => {
                    out.push('\\');
                    out.push(other);
                    i += 2;
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(ChefData::from_text(&out))
}

// ---------------------------------------------------------------------------
// 字符集转码（GBK / GB18030 / Big5 / Shift_JIS / Windows-1251 …）
// ---------------------------------------------------------------------------

const CHARSETS: &[&str] = &[
    "utf-8",
    "gbk",
    "gb18030",
    "gb2312",
    "big5",
    "shift_jis",
    "euc-jp",
    "euc-kr",
    "windows-1251",
    "windows-1252",
    "iso-8859-2",
    "koi8-r",
    "utf-16le",
    "utf-16be",
    "latin1",
];

fn encoding_for(name: &str) -> ChefResult<&'static encoding_rs::Encoding> {
    encoding_rs::Encoding::for_label(name.as_bytes())
        .ok_or_else(|| fail(format!("不支持的字符集「{name}」")))
}

pub fn charset_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let name = args.choice_in("charset", "gbk", CHARSETS)?;
    let encoding = encoding_for(&name)?;
    let (bytes, _actual, had_errors) = encoding.encode(data.text()?);
    if had_errors && args.bool_of("strict") {
        return Err(fail(format!(
            "{name} 无法表示这些字符（出现了 ? 替换）。改字符集或去掉严格模式"
        )));
    }
    Ok(ChefData::new(bytes.into_owned()))
}

pub fn charset_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let name = args.choice_in("charset", "gbk", CHARSETS)?;
    let encoding = encoding_for(&name)?;
    let (text, _actual, had_errors) = encoding.decode(&data.bytes);
    if had_errors && args.bool_of("strict") {
        return Err(fail(format!(
            "{name} 解码时出现非法字节（结果里有替换符）。换字符集或去掉严格模式"
        )));
    }
    let mut out = text.into_owned();
    if args.bool_of("stripBom") && out.starts_with('\u{feff}') {
        out.remove(0);
    }
    Ok(ChefData::from_text(&out))
}

// ---------------------------------------------------------------------------
// 莫尔斯电码
// ---------------------------------------------------------------------------

const MORSE: &[(&str, &str)] = &[
    ("A", ".-"),
    ("B", "-..."),
    ("C", "-.-."),
    ("D", "-.."),
    ("E", "."),
    ("F", "..-."),
    ("G", "--."),
    ("H", "...."),
    ("I", ".."),
    ("J", ".---"),
    ("K", "-.-"),
    ("L", ".-.."),
    ("M", "--"),
    ("N", "-."),
    ("O", "---"),
    ("P", ".--."),
    ("Q", "--.-"),
    ("R", ".-."),
    ("S", "..."),
    ("T", "-"),
    ("U", "..-"),
    ("V", "...-"),
    ("W", ".--"),
    ("X", "-..-"),
    ("Y", "-.--"),
    ("Z", "--.."),
    ("0", "-----"),
    ("1", ".----"),
    ("2", "..---"),
    ("3", "...--"),
    ("4", "....-"),
    ("5", "....."),
    ("6", "-...."),
    ("7", "--..."),
    ("8", "---.."),
    ("9", "----."),
    (".", ".-.-.-"),
    (",", "--..--"),
    ("?", "..--.."),
    ("'", ".----."),
    ("!", "-.-.--"),
    ("/", "-..-."),
    ("(", "-.--."),
    (")", "-.--.-"),
    ("&", ".-..."),
    (":", "---..."),
    (";", "-.-.-."),
    ("=", "-...-"),
    ("+", ".-.-."),
    ("-", "-....-"),
    ("_", "..--.-"),
    ("\"", ".-..-."),
    ("$", "...-..-"),
    ("@", ".--.-."),
];

pub fn morse_encode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let letter_sep = args.text("letterSeparator", " ");
    let word_sep = args.text("wordSeparator", "/");
    let mut words: Vec<String> = Vec::new();
    for word in data.text()?.split_whitespace() {
        let mut parts: Vec<String> = Vec::new();
        for ch in word.chars() {
            let upper = ch.to_ascii_uppercase();
            match MORSE.iter().find(|(c, _)| *c == upper.to_string()) {
                Some((_, code)) => parts.push((*code).to_string()),
                None => {
                    return Err(fail(format!(
                        "「{ch}」没有莫尔斯编码（这个表只覆盖拉丁字母、数字与常用标点）"
                    )));
                }
            }
        }
        words.push(parts.join(&letter_sep));
    }
    Ok(ChefData::from_text(&words.join(&format!(" {word_sep} "))))
}

pub fn morse_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let word_sep = args.text("wordSeparator", "/");
    let mut out = String::new();
    for word in data.text()?.split(word_sep.as_str()) {
        let mut decoded = String::new();
        for code in word.split_whitespace() {
            let matched = MORSE.iter().find(|(_, c)| *c == code);
            match matched {
                Some((ch, _)) => decoded.push_str(ch),
                None => return Err(fail(format!("未知莫尔斯码「{code}」"))),
            }
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&decoded);
    }
    Ok(ChefData::from_text(&out))
}

pub const OPS: &[super::Op] = &[
    op(
        "base64Encode",
        "encoding",
        &[bool_arg("urlSafe", false), bool_arg("noWrap", true)],
        base64_encode,
    ),
    op(
        "base64Decode",
        "encoding",
        &[bool_arg("urlSafe", false), bool_arg("strict", false)],
        base64_decode,
    ),
    op(
        "base32Encode",
        "encoding",
        &[bool_arg("pad", true), bool_arg("hexAlphabet", false)],
        base32_encode,
    ),
    op(
        "base32Decode",
        "encoding",
        &[bool_arg("hexAlphabet", false)],
        base32_decode,
    ),
    op(
        "base58Encode",
        "encoding",
        &[choice(
            "alphabet",
            "bitcoin",
            &["bitcoin", "ripple", "flickr"],
        )],
        base58_encode,
    ),
    op(
        "base58Decode",
        "encoding",
        &[choice(
            "alphabet",
            "bitcoin",
            &["bitcoin", "ripple", "flickr"],
        )],
        base58_decode,
    ),
    op("base62Encode", "encoding", &[], base62_encode),
    op("base62Decode", "encoding", &[], base62_decode),
    op(
        "base85Encode",
        "encoding",
        &[
            choice("style", "adobe", BASE85_STYLES),
            bool_arg("delimiters", true),
        ],
        base85_encode,
    ),
    op(
        "base85Decode",
        "encoding",
        &[choice("style", "adobe", BASE85_STYLES)],
        base85_decode,
    ),
    op(
        "hexEncode",
        "encoding",
        &[bool_arg("uppercase", false), text("separator", "")],
        hex_encode_op,
    ),
    op("hexDecode", "encoding", &[], hex_decode_op),
    op(
        "decimalEncode",
        "encoding",
        &[text("separator", " ")],
        decimal_encode,
    ),
    op(
        "decimalDecode",
        "encoding",
        &[text("separator", " ")],
        decimal_decode,
    ),
    op(
        "octalEncode",
        "encoding",
        &[text("separator", " ")],
        octal_encode,
    ),
    op(
        "octalDecode",
        "encoding",
        &[text("separator", " ")],
        octal_decode,
    ),
    op(
        "binaryEncode",
        "encoding",
        &[text("separator", " ")],
        binary_encode,
    ),
    op(
        "binaryDecode",
        "encoding",
        &[text("separator", " ")],
        binary_decode,
    ),
    op(
        "urlEncode",
        "encoding",
        &[bool_arg("component", false), bool_arg("uppercase", true)],
        url_encode,
    ),
    op(
        "urlDecode",
        "encoding",
        &[bool_arg("plusAsSpace", true)],
        url_decode,
    ),
    op(
        "htmlEntityEncode",
        "encoding",
        &[choice(
            "style",
            "named",
            &["named", "decimal", "hex", "all-non-ascii"],
        )],
        html_entity_encode,
    ),
    op("htmlEntityDecode", "encoding", &[], html_entity_decode),
    op(
        "unicodeEscape",
        "encoding",
        &[bool_arg("nonAsciiOnly", true)],
        unicode_escape,
    ),
    op("unicodeUnescape", "encoding", &[], unicode_unescape),
    op(
        "charsetEncode",
        "encoding",
        &[
            choice("charset", "gbk", CHARSETS),
            bool_arg("strict", false),
        ],
        charset_encode,
    ),
    op(
        "charsetDecode",
        "encoding",
        &[
            choice("charset", "gbk", CHARSETS),
            bool_arg("strict", false),
            bool_arg("stripBom", true),
        ],
        charset_decode,
    ),
    op(
        "morseEncode",
        "encoding",
        &[text("letterSeparator", " "), text("wordSeparator", "/")],
        morse_encode,
    ),
    op(
        "morseDecode",
        "encoding",
        &[text("wordSeparator", "/")],
        morse_decode,
    ),
];

#[cfg(test)]
mod tests {
    use crate::services::chef::testing::{expect_error, run_op, run_op_bytes, run_op_text};

    /// 一律走注册表：默认值来自 `spec`，测试与生产用同一份参数装配路径
    fn text(id: &str, input: &str, overrides: &[(&str, &str)]) -> String {
        run_op_text(id, input, overrides)
    }

    fn bytes(id: &str, input: &str, overrides: &[(&str, &str)]) -> Vec<u8> {
        run_op(id, input, overrides).bytes
    }

    #[test]
    fn base64_vectors_and_roundtrip() {
        assert_eq!(text("base64Encode", "hello 世界", &[]), "aGVsbG8g5LiW55WM");
        assert_eq!(text("base64Decode", "aGVsbG8g5LiW55WM", &[]), "hello 世界");
        // URL-safe：+ / 换成 - _（0xfb 0xff 这两字节正好落在 62/63 两个位置上）
        let encoded =
            run_op_bytes("base64Encode", &[0xfb, 0xff], &[("urlSafe", "true")]).text_lossy();
        assert_eq!(encoded, "-_8=");
        assert_eq!(
            run_op_bytes("base64Decode", encoded.as_bytes(), &[("urlSafe", "true")]).bytes,
            vec![0xfb, 0xff]
        );
        // 标准字母表同样两字节给的是 + /
        assert_eq!(
            run_op_bytes("base64Encode", &[0xfb, 0xff], &[]).text_lossy(),
            "+/8="
        );
        // 宽松解码要把两种写法互相兜住
        assert_eq!(
            run_op_bytes("base64Decode", b"+/8=", &[]).bytes,
            vec![0xfb, 0xff]
        );
        assert_eq!(
            run_op_bytes("base64Decode", b"-_8=", &[]).bytes,
            vec![0xfb, 0xff]
        );
        // 默认不换行；关掉 noWrap 就按 76 字符断行（PEM 里那种）
        let long = "A".repeat(120);
        assert!(!text("base64Encode", &long, &[]).contains('\n'));
        let wrapped = text("base64Encode", &long, &[("noWrap", "false")]);
        assert!(wrapped.contains('\n'), "{wrapped}");
        assert_eq!(wrapped.lines().next().unwrap().len(), 76);
        // 默认保留 padding
        assert_eq!(text("base64Encode", "f", &[]), "Zg==");
    }

    #[test]
    fn base64_strict_needs_padding() {
        assert_eq!(text("base64Decode", "aGVsbG8", &[]), "hello");
        let message = expect_error("base64Decode", "aGVsbG8", &[("strict", "true")]);
        assert!(message.contains("Base64"), "{message}");
    }

    #[test]
    fn base32_vectors() {
        // RFC 4648 §10 的示例表
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY======"),
            ("fo", "MZXQ===="),
            ("foo", "MZXW6==="),
            ("foob", "MZXW6YQ="),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI======"),
        ] {
            assert_eq!(text("base32Encode", plain, &[]), encoded, "编码 {plain:?}");
            assert_eq!(
                text("base32Decode", encoded, &[]),
                plain,
                "解码 {encoded:?}"
            );
            // 不带填充也能解
            assert_eq!(
                text("base32Encode", plain, &[("pad", "false")]),
                encoded.trim_end_matches('=')
            );
        }
        // Base32hex 换字母表：同一个 "foo" 给 CPNMU===
        assert_eq!(
            text("base32Encode", "foo", &[("hexAlphabet", "true")]),
            "CPNMU==="
        );
        assert_eq!(
            text("base32Decode", "CPNMU===", &[("hexAlphabet", "true")]),
            "foo"
        );
    }

    #[test]
    fn base58_and_base62_are_big_integer_radices() {
        // Bitcoin 字母表：前导 0 字节变成前导 '1'
        assert_eq!(text("base58Encode", "\u{0}\u{0}hi", &[]), {
            let inner = text("base58Encode", "hi", &[]);
            format!("11{inner}")
        });
        assert_eq!(
            text(
                "base58Decode",
                &text("base58Encode", "hello world", &[]),
                &[]
            ),
            "hello world"
        );
        assert_eq!(text("base58Encode", "hello world", &[]), "StV1DL6CwTryKyV");
        assert_eq!(
            text(
                "base58Decode",
                &text("base58Encode", "\u{0}\u{0}\u{7}x", &[]),
                &[]
            ),
            "\u{0}\u{0}\u{7}x"
        );
        for sample in ["", "a", "hello world", "\u{0}\u{0}\u{0}z"] {
            assert_eq!(
                text("base62Decode", &text("base62Encode", sample, &[]), &[]),
                sample
            );
        }
        let message = expect_error("base58Decode", "0OIl", &[]);
        assert!(message.contains("字母表"), "{message}");
    }

    #[test]
    fn base85_vectors() {
        // Adobe Ascii85（字符 33..117）：Python 的 base64.a85encode 同一口径
        for (plain, encoded) in [
            ("a", "@/"),
            ("ab", "@:B"),
            ("abc", "@:E^"),
            ("abcd", "@:E_W"),
            ("hello world!", "BOu!rD]j7BEbo80"),
        ] {
            assert_eq!(
                run_op_bytes(
                    "base85Encode",
                    plain.as_bytes(),
                    &[("style", "adobe"), ("delimiters", "false")]
                )
                .text_lossy(),
                encoded,
                "Adobe 编码 {plain:?}"
            );
            assert_eq!(
                run_op_bytes("base85Decode", encoded.as_bytes(), &[("style", "adobe")]).bytes,
                plain.as_bytes(),
                "Adobe 解码 {encoded:?}"
            );
        }
        // 四个零字节 → z 缩写；带定界符时外面是 <~ ~>
        assert_eq!(
            run_op_bytes(
                "base85Encode",
                &[0, 0, 0, 0],
                &[("style", "adobe"), ("delimiters", "false")]
            )
            .text_lossy(),
            "z"
        );
        assert_eq!(
            run_op_bytes("base85Encode", &[0, 0, 0, 0], &[("style", "adobe")]).text_lossy(),
            "<~z~>"
        );
        assert_eq!(
            run_op_bytes("base85Decode", b"<~z~>", &[("style", "adobe")]).bytes,
            vec![0, 0, 0, 0]
        );
        // RFC 1924 字母表（Base85 / Z85）：同一串数据与 Adobe 完全不同
        for (plain, encoded) in [
            ("a", "VE"),
            ("ab", "VPX"),
            ("abc", "VPaz"),
            ("abcd", "VPa!s"),
            ("abcde", "VPa!sWd"),
            ("hello world!", "Xk~0{Zy<MXa%^NF"),
        ] {
            assert_eq!(
                run_op_bytes("base85Encode", plain.as_bytes(), &[("style", "base85")]).text_lossy(),
                encoded,
                "Base85 编码 {plain:?}"
            );
            assert_eq!(
                run_op_bytes("base85Decode", encoded.as_bytes(), &[("style", "base85")]).bytes,
                plain.as_bytes(),
                "Base85 解码 {encoded:?}"
            );
        }
        // Z85 只接受整组 5 字符（libzmq 的口径）：4 字节 ↔ 5 字符，尾组直接拒绝
        assert_eq!(
            run_op_bytes("base85Decode", b"VPa!s", &[("style", "z85")]).bytes,
            b"abcd"
        );
        let message = expect_error("base85Decode", "VPa!sWd", &[("style", "z85")]);
        assert!(message.contains("Z85"), "{message}");
        // 往返：任意长度都要保持字节不变
        for len in 0..12usize {
            let sample: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            for style in ["adobe", "base85"] {
                let encoded = run_op_bytes(
                    "base85Encode",
                    &sample,
                    &[("style", style), ("delimiters", "false")],
                )
                .bytes;
                let back = run_op_bytes("base85Decode", &encoded, &[("style", style)]).bytes;
                assert_eq!(back, sample, "风格 {style} 长度 {len} 往返不一致");
            }
        }
    }

    #[test]
    fn hex_and_number_lists() {
        assert_eq!(text("hexEncode", "ab", &[]), "6162");
        assert_eq!(
            text(
                "hexEncode",
                "ab",
                &[("uppercase", "true"), ("separator", " ")]
            ),
            "61 62"
        );
        assert_eq!(bytes("hexDecode", "61 62", &[]), b"ab");
        // 奇数长度左侧补 0（与 CyberChef「From Hex」一致）
        assert_eq!(bytes("hexDecode", "f00b", &[]), vec![0xf0, 0x0b]);
        assert_eq!(text("decimalEncode", "ab", &[]), "97 98");
        assert_eq!(bytes("decimalDecode", "97 98", &[]), b"ab");
        assert_eq!(text("binaryEncode", "A", &[]), "01000001");
        assert_eq!(bytes("binaryDecode", "01000001", &[]), b"A");
        assert_eq!(text("octalEncode", "A", &[]), "101");
        assert_eq!(bytes("octalDecode", "101", &[]), b"A");
        let message = expect_error("decimalDecode", "256", &[]);
        assert!(message.contains("超出一个字节"), "{message}");
    }

    #[test]
    fn url_percent_coding() {
        assert_eq!(text("urlEncode", "a b=中", &[]), "a%20b%3D%E4%B8%AD");
        assert_eq!(
            text("urlEncode", "a b='!*()", &[("component", "true")]),
            "a%20b%3D'!*()"
        );
        assert_eq!(text("urlEncode", "a b", &[("uppercase", "false")]), "a%20b");
        assert_eq!(text("urlDecode", "a%20b%3D%E4%B8%AD", &[]), "a b=中");
        assert_eq!(text("urlDecode", "a+b", &[]), "a b");
        assert_eq!(text("urlDecode", "a+b", &[("plusAsSpace", "false")]), "a+b");
        // 解出非 UTF-8 字节时，消息要指路到字符集解码
        let message = expect_error("urlDecode", "%C4%E3", &[]);
        assert!(message.contains("GBK"), "{message}");
    }

    #[test]
    fn html_entities() {
        assert_eq!(text("htmlEntityEncode", "<a>&", &[]), "&lt;a&gt;&amp;");
        assert_eq!(
            text("htmlEntityEncode", "中", &[("style", "hex")]),
            "&#x4e2d;"
        );
        assert_eq!(
            text("htmlEntityEncode", "中", &[("style", "decimal")]),
            "&#20013;"
        );
        assert_eq!(
            text("htmlEntityDecode", "&lt;a&gt;&amp;&#65;&#x42;&nbsp;", &[]),
            "<a>&AB\u{a0}"
        );
        // 词典里没有的实体原样留着，不能吞字符
        assert_eq!(
            text("htmlEntityDecode", "&unknown; &lt;", &[]),
            "&unknown; <"
        );
        assert_eq!(text("htmlEntityDecode", "a & b", &[]), "a & b");
    }

    #[test]
    fn unicode_escapes() {
        assert_eq!(text("unicodeEscape", "中a", &[]), "\\u4e2da");
        assert_eq!(text("unicodeUnescape", "\\u4e2da", &[]), "中a");
        // Java 风格代理对
        assert_eq!(text("unicodeUnescape", "\\ud83d\\ude00", &[]), "\u{1f600}");
        assert_eq!(text("unicodeEscape", "\u{1f600}", &[]), "\\ud83d\\ude00");
        assert_eq!(text("unicodeUnescape", "\\n\\t\\\\", &[]), "\n\t\\");
        // nonAsciiOnly=false 时 ASCII 也转义
        assert_eq!(
            text("unicodeEscape", "ab", &[("nonAsciiOnly", "false")]),
            "\\u0061\\u0062"
        );
    }

    #[test]
    fn charset_recode() {
        // GBK：中文两个字节
        assert_eq!(
            bytes("charsetEncode", "中文", &[]),
            vec![0xd6, 0xd0, 0xce, 0xc4]
        );
        assert_eq!(
            run_op_bytes("charsetDecode", &[0xd6, 0xd0, 0xce, 0xc4], &[]).text_lossy(),
            "中文"
        );
        assert_eq!(
            bytes("charsetEncode", "中文", &[("charset", "utf-8")]),
            "中文".as_bytes()
        );
        // 严格模式必须拒绝 GBK 表示不了的字符，而不是悄悄给一串 ?
        let message = expect_error(
            "charsetEncode",
            "\u{1f600}",
            &[("charset", "gbk"), ("strict", "true")],
        );
        assert!(message.contains("无法表示"), "{message}");
        assert_eq!(
            run_op_bytes("charsetDecode", &[0xe9], &[("charset", "latin1")]).text_lossy(),
            "\u{e9}"
        );
        // Big5 与 GBK 对同一字节序列给出不同汉字：证明字符集参数真的生效
        assert_ne!(
            run_op_bytes("charsetDecode", &[0xa4, 0xa4], &[("charset", "big5")]).text_lossy(),
            run_op_bytes("charsetDecode", &[0xa4, 0xa4], &[("charset", "gbk")]).text_lossy()
        );
    }

    #[test]
    fn morse_roundtrip() {
        assert_eq!(text("morseEncode", "SOS", &[]), "... --- ...");
        assert_eq!(text("morseDecode", "... --- ...", &[]), "SOS");
        assert_eq!(text("morseEncode", "A B", &[]), ".- / -...");
        assert_eq!(text("morseDecode", ".- / -...", &[]), "A B");
        assert_eq!(
            text("morseEncode", "SOS", &[("wordSeparator", " | ")]),
            "... --- ..."
        );
        let message = expect_error("morseEncode", "中", &[]);
        assert!(message.contains("莫尔斯"), "{message}");
    }
}
