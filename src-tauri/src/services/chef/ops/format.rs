//! 结构化解析：JSON、JWT、protobuf wire、DER/ASN.1、十六进制转储、UUID、文件类型。
//!
//! 这一组是「看着一团乱码，先解析成能读的东西」的入口。解析失败必须说清
//! 为什么失败（截断？不是 JSON？字节序不对？），不要返回空字符串糊弄界面。

use serde_json::Value;

use crate::services::chef::data::{
    Args, ChefData, ChefError, ChefResult, b64_decode, fail, hex_encode,
};
use crate::services::chef::spec::{bool_arg, choice, number, op, text};
use base64::Engine;

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn parse_json(data: &ChefData) -> ChefResult<Value> {
    let text = data.text()?;
    serde_json::from_str(text.trim()).map_err(|e| {
        fail(format!(
            "不是合法 JSON：{e}。JS 对象字面量（键没引号、带注释、带尾逗号）需要先手工改成标准 JSON"
        ))
    })
}

fn json_beautify(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let value = parse_json(data)?;
    let indent = args.number("indent", 2)?.clamp(1, 8) as usize;
    let pad = " ".repeat(indent);
    let mut out = String::new();
    write_pretty(&value, &mut out, &pad, 0);
    Ok(ChefData::from_text(&out))
}

/// 自己控制缩进宽度：serde_json 的 pretty 固定 2 空格，跟着项目风格走就要接管
fn write_pretty(value: &Value, out: &mut String, pad: &str, depth: usize) {
    let inner = pad.repeat(depth + 1);
    let close = pad.repeat(depth);
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, child)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_pretty(child, out, pad, depth + 1);
            }
            out.push('\n');
            out.push_str(&close);
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, child) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                write_pretty(child, out, pad, depth + 1);
            }
            out.push('\n');
            out.push_str(&close);
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn json_minify(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(&parse_json(data)?.to_string()))
}

// ---------------------------------------------------------------------------
// JWT
// ---------------------------------------------------------------------------

fn split_jwt(data: &ChefData) -> ChefResult<(String, String, String)> {
    let token = data.text()?.trim().to_string();
    let token = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .map(str::to_string)
        .unwrap_or(token);
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(fail(format!(
            "不是一枚三段式 JWT（按 . 切出来 {} 段）。别把 Bearer 前缀或引号一起粘进来",
            parts.len()
        )));
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    ))
}

fn jwt_segment_json(raw: &str, what: &str) -> ChefResult<Value> {
    let bytes = b64_decode(raw, true, false)
        .map_err(|e| fail(format!("JWT 的{what}段不是合法 Base64URL：{e}")))?;
    serde_json::from_slice(&bytes).map_err(|e| fail(format!("JWT 的{what}段解出来不是 JSON：{e}")))
}

fn jwt_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let (header, payload, signature) = split_jwt(data)?;
    let header = jwt_segment_json(&header, "头")?;
    let mut payload_json = jwt_segment_json(&payload, "载荷")?;
    let indent = args.number("indent", 2)?.clamp(1, 8) as usize;
    let pad = " ".repeat(indent);
    let mut out = String::new();
    out.push_str("header:\n");
    write_pretty(&header, &mut out, &pad, 0);
    out.push_str("\n\npayload:\n");
    write_pretty(&payload_json, &mut out, &pad, 0);
    if args.bool_of("humanReadableDates") {
        // exp/iat/nbf 顺手换成可读时间：JWT 里这三个字段人人要看
        for key in ["iat", "exp", "nbf"] {
            if let Some(value) = payload_json.get(key).and_then(Value::as_i64) {
                if let Some(text) = crate::services::chef::ops::text::format_unix(value, 0) {
                    if let Some(map) = payload_json.as_object_mut() {
                        map.insert(format!("{key}Text"), Value::String(text));
                    }
                }
            }
        }
        out.clear();
        out.push_str("header:\n");
        write_pretty(&header, &mut out, &pad, 0);
        out.push_str("\n\npayload:\n");
        write_pretty(&payload_json, &mut out, &pad, 0);
    }
    let algorithm = header
        .get("alg")
        .and_then(Value::as_str)
        .unwrap_or("(缺 alg)")
        .to_string();
    out.push_str(&format!(
        "\n\nsignature:\n{signature}\n\n签名算法：{algorithm}"
    ));
    let secret = args.text("secret", "");
    if !secret.is_empty() {
        out.push_str(&format!(
            "\n验签：{}",
            verify_hmac(
                &algorithm,
                &format!(
                    "{header_b64}.{payload_b64}",
                    header_b64 = split_jwt(data)?.0,
                    payload_b64 = split_jwt(data)?.1
                ),
                &signature,
                &secret,
            )?
        ));
    }
    Ok(ChefData::from_text(&out))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 用密钥验 HMAC 系列签名；其它算法明确说「这里验不了」而不是回一句「失败」
fn verify_hmac(
    algorithm: &str,
    signing_input: &str,
    signature: &str,
    secret: &str,
) -> ChefResult<String> {
    use hmac::Mac;
    let expected = match algorithm {
        "HS256" => {
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|_| fail("内部：HMAC 密钥装配失败"))?;
            mac.update(signing_input.as_bytes());
            b64url(&mac.finalize().into_bytes())
        }
        "HS384" => {
            let mut mac = hmac::Hmac::<sha2::Sha384>::new_from_slice(secret.as_bytes())
                .map_err(|_| fail("内部：HMAC 密钥装配失败"))?;
            mac.update(signing_input.as_bytes());
            b64url(&mac.finalize().into_bytes())
        }
        "HS512" => {
            let mut mac = hmac::Hmac::<sha2::Sha512>::new_from_slice(secret.as_bytes())
                .map_err(|_| fail("内部：HMAC 密钥装配失败"))?;
            mac.update(signing_input.as_bytes());
            b64url(&mac.finalize().into_bytes())
        }
        other => return Ok(format!("跳过（{other} 不是 HMAC 系列，这里只能解出内容）")),
    };
    Ok(if expected == signature {
        "通过：这个密钥确实签出了这枚 token".to_string()
    } else {
        format!("不通过：按 {signature} 算出来应该是 {expected}")
    })
}

// ---------------------------------------------------------------------------
// 十六进制转储
// ---------------------------------------------------------------------------

fn hex_dump(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let style = args.choice_in("style", "xxd", &["xxd", "hexdump", "plain"])?;
    let width = args.number("bytesPerLine", 16)?.clamp(4, 64) as usize;
    let mut out = String::new();
    for (index, chunk) in data.bytes.chunks(width).enumerate() {
        let offset = index * width;
        match style.as_str() {
            "plain" => {
                out.push_str(&hex_encode(chunk, false, ""));
                out.push('\n');
            }
            "hexdump" => {
                out.push_str(&format!("{offset:08x} "));
                for (i, byte) in chunk.iter().enumerate() {
                    out.push_str(&format!("{byte:02x} "));
                    if i + 1 == width / 2 {
                        out.push(' ');
                    }
                }
                for _ in chunk.len()..width {
                    out.push_str("   ");
                }
                out.push_str(&format!(
                    " |{}|",
                    chunk
                        .iter()
                        .map(|b| if b.is_ascii_graphic() || *b == b' ' {
                            *b as char
                        } else {
                            '.'
                        })
                        .collect::<String>()
                ));
                out.push('\n');
            }
            _ => {
                out.push_str(&format!("{offset:08x}: "));
                let mut hex = String::new();
                for (i, byte) in chunk.iter().enumerate() {
                    hex.push_str(&format!("{byte:02x}"));
                    if i + 1 < chunk.len() {
                        hex.push(' ');
                    }
                    if i + 1 == width / 2 {
                        hex.push(' ');
                    }
                }
                let ascii: String = chunk
                    .iter()
                    .map(|b| {
                        if b.is_ascii_graphic() || *b == b' ' {
                            *b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                out.push_str(&format!("{hex:<44}  {ascii}\n"));
            }
        }
    }
    if data.bytes.is_empty() {
        return Err(fail("没有内容可转储（输入框是空的）"));
    }
    out.push_str(&format!("\n共 {} 字节", data.bytes.len()));
    Ok(ChefData::from_text(&out))
}

// ---------------------------------------------------------------------------
// protobuf wire format（没有 .proto 也能先读出结构）
// ---------------------------------------------------------------------------

fn read_varint(bytes: &[u8], cursor: &mut usize) -> ChefResult<u64> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| fail("protobuf 数据在 varint 中间被截断"))?;
        *cursor += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(fail(
                "protobuf varint 超过 64 位，这段数据大概不是 protobuf",
            ));
        }
    }
}

fn protobuf_fields(
    bytes: &[u8],
    depth: usize,
    prefer_text: bool,
    out: &mut String,
) -> ChefResult<()> {
    const MAX_DEPTH: usize = 8;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let key = read_varint(bytes, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == 0 {
            return Err(fail(
                "protobuf 出现了 0 号字段：这段数据大概不是 protobuf wire 格式",
            ));
        }
        let pad = "  ".repeat(depth);
        match wire {
            0 => {
                let value = read_varint(bytes, &mut cursor)?;
                out.push_str(&format!("{pad}{field} varint {value}\n"));
            }
            1 => {
                if cursor + 8 > bytes.len() {
                    return Err(fail("protobuf 的 64 位定长字段被截断"));
                }
                let raw = &bytes[cursor..cursor + 8];
                cursor += 8;
                out.push_str(&format!(
                    "{pad}{field} fixed64 {}\n",
                    u64::from_le_bytes(raw.try_into().expect("8 字节"))
                ));
            }
            5 => {
                if cursor + 4 > bytes.len() {
                    return Err(fail("protobuf 的 32 位定长字段被截断"));
                }
                let raw = &bytes[cursor..cursor + 4];
                cursor += 4;
                out.push_str(&format!(
                    "{pad}{field} fixed32 {}\n",
                    u32::from_le_bytes(raw.try_into().expect("4 字节"))
                ));
            }
            2 => {
                let len = read_varint(bytes, &mut cursor)? as usize;
                if cursor + len > bytes.len() {
                    return Err(fail(format!(
                        "protobuf 长度前缀说这里有 {len} 字节，实际只剩 {}\n（多半是截断过的包，或字段号对不上）",
                        bytes.len() - cursor
                    )));
                }
                let body = &bytes[cursor..cursor + len];
                cursor += len;
                // wire 格式天生有歧义：一段字节既可能是字符串也可能是嵌套 message。
                // 默认按文本优先（逆向时绝大多数是字符串），关掉 preferText 才强解成消息。
                let clean_text = std::str::from_utf8(body).ok().filter(|text| {
                    !text.is_empty()
                        && text
                            .chars()
                            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
                });
                let can_nest = depth < MAX_DEPTH && !body.is_empty();
                if clean_text.is_some() && (prefer_text || !can_nest) {
                    let text = clean_text.unwrap_or_default();
                    out.push_str(&format!("{pad}{field} string {text:?}"));
                } else if can_nest {
                    out.push_str(&format!("{pad}{field} message({len} 字节)\n"));
                    protobuf_fields(body, depth + 1, prefer_text, out)?;
                } else {
                    out.push_str(&format!(
                        "{pad}{field} bytes({len}): {}\n",
                        hex_encode(body, false, " ")
                    ));
                }
            }
            3 | 4 => {
                return Err(fail(
                    "这段数据里有已经废弃的 group 编码（wire type 3/4），当前解不了",
                ));
            }
            other => return Err(fail(format!("未知的 wire type {other}"))),
        }
    }
    Ok(())
}

fn protobuf_decode(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    if data.bytes.is_empty() {
        return Err(fail("没有内容可解析（输入框是空的）"));
    }
    let mut out = String::new();
    protobuf_fields(&data.bytes, 0, args.bool_of("preferText"), &mut out)
        .map_err(|e: ChefError| fail(format!("protobuf 解析失败：{}", e.message)))?;
    Ok(ChefData::from_text(&out))
}

// ---------------------------------------------------------------------------
// DER / ASN.1（证书、签名块、密钥文件）
// ---------------------------------------------------------------------------

fn der_tree(bytes: &[u8], depth: usize, out: &mut String) -> ChefResult<usize> {
    const MAX_DEPTH: usize = 12;
    if depth > MAX_DEPTH {
        return Err(fail("ASN.1 嵌套超过 12 层，先怀疑这不是 DER"));
    }
    let mut consumed = 0usize;
    while consumed < bytes.len() {
        let start = consumed;
        let first = bytes[consumed];
        consumed += 1;
        if consumed >= bytes.len() {
            return Err(fail("ASN.1 数据在长度字段之前就结束了"));
        }
        let mut length = bytes[consumed];
        consumed += 1;
        if length & 0x80 != 0 {
            let count = (length & 0x7f) as usize;
            if count == 0 || count > 8 || consumed + count > bytes.len() {
                return Err(fail("ASN.1 长度字段不合法"));
            }
            let mut value = 0usize;
            for byte in &bytes[consumed..consumed + count] {
                value = (value << 8) | *byte as usize;
            }
            consumed += count;
            length = value as u8; // 仅用于短数据时的显示，长数据用 value_len 变量
            let _ = length;
            // 重新用一个 usize 变量表示内容长度，避免上面 u8 的截断
            let content_len = value;
            if consumed + content_len > bytes.len() {
                return Err(fail(format!(
                    "ASN.1 说这段有 {content_len} 字节，实际只剩 {}",
                    bytes.len() - consumed
                )));
            }
            let inner = &bytes[consumed..consumed + content_len];
            out.push_str(&der_line(first, content_len, inner, depth));
            if first & 0x20 != 0 {
                der_tree(inner, depth + 1, out)?;
            }
            consumed += content_len;
        } else {
            let content_len = length as usize;
            if consumed + content_len > bytes.len() {
                return Err(fail(format!(
                    "ASN.1 说这段有 {content_len} 字节，实际只剩 {}",
                    bytes.len() - consumed
                )));
            }
            let inner = &bytes[consumed..consumed + content_len];
            out.push_str(&der_line(first, content_len, inner, depth));
            if first & 0x20 != 0 {
                der_tree(inner, depth + 1, out)?;
            }
            consumed += content_len;
        }
        let _ = start;
    }
    Ok(consumed)
}

fn tag_name(tag: u8) -> &'static str {
    if tag >> 6 != 0 {
        return "context tag";
    }
    match tag & 0x1f {
        0x01 => "BOOLEAN",
        0x02 => "INTEGER",
        0x03 => "BIT STRING",
        0x04 => "OCTET STRING",
        0x05 => "NULL",
        0x06 => "OBJECT IDENTIFIER",
        0x0c => "UTF8String",
        0x10 => "SEQUENCE",
        0x11 => "SET",
        0x13 => "PrintableString",
        0x14 => "T61String",
        0x16 => "IA5String",
        0x17 => "UTCTime",
        0x18 => "GeneralizedTime",
        _ => "tag",
    }
}

/// OID 的点分写法：DER 里是 128 进制多字节，不解出来根本认不出 rsaEncryption
fn dotted_oid(bytes: &[u8]) -> Option<String> {
    let first = *bytes.first()?;
    let mut out = vec![(first / 40).to_string(), (first % 40).to_string()];
    let mut value: u64 = 0;
    for byte in &bytes[1..] {
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            out.push(value.to_string());
            value = 0;
        }
    }
    if value != 0 {
        return None;
    }
    Some(out.join("."))
}

/// 叶子的可读值：整数给十六进制、OID 给点分、字符串给文本
fn leaf_value(tag: u8, content: &[u8]) -> Option<String> {
    match tag & 0x1f {
        0x02 => Some(
            content
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        ),
        0x06 => dotted_oid(content),
        0x0c | 0x13 | 0x16 | 0x17 | 0x18 => std::str::from_utf8(content).ok().map(str::to_string),
        _ => None,
    }
}

fn der_line(tag: u8, length: usize, content: &[u8], depth: usize) -> String {
    let pad = "  ".repeat(depth);
    let prefix = match tag >> 6 {
        0 => String::new(),
        1 => "[APPLICATION] ".to_string(),
        2 => "[CONTEXT] ".to_string(),
        _ => "[PRIVATE] ".to_string(),
    };
    let mut line = format!("{pad}{prefix}{} ({length} 字节)", tag_name(tag));
    if tag & 0x20 == 0 {
        if let Some(value) = leaf_value(tag, content) {
            line.push_str(&format!(" = {value}"));
        }
    }
    line.push('\n');
    line
}

fn der_decode(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    if data.bytes.is_empty() {
        return Err(fail("没有内容可解析（输入框是空的）"));
    }
    let mut out = String::new();
    der_tree(&data.bytes, 0, &mut out).map_err(|e| fail(format!("DER 解析失败：{}", e.message)))?;
    Ok(ChefData::from_text(&out))
}

// ---------------------------------------------------------------------------
// UUID 与文件类型
// ---------------------------------------------------------------------------

fn parse_uuid(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let text = data.text()?.trim().to_lowercase().replace("-", "");
    let bytes = hex::decode(&text).map_err(|_| fail("UUID 要是 32 个十六进制字符"))?;
    if bytes.len() != 16 {
        return Err(fail(format!(
            "UUID 是 16 字节，这里给了 {} 字节",
            bytes.len()
        )));
    }
    let variant = match bytes[8] >> 5 {
        0b110 => "RFC 4122",
        0b111 => "Microsoft",
        0b11 => "Reserved",
        _ => "NCS",
    };
    let version = bytes[6] >> 4;
    let known = match version {
        1 => "基于时间",
        2 => "DCE 安全",
        3 => "MD5 名字空间",
        4 => "随机",
        5 => "SHA-1 名字空间",
        6 => "基于时间（重排）",
        7 => "Unix 毫秒时间戳",
        other => return Ok(ChefData::from_text(&format!("未定义的版本号 {other}"))),
    };
    Ok(ChefData::from_text(&format!(
        "{}\n版本 {version}（{known}）· 变体 {variant}",
        uuid::Builder::from_bytes(bytes.try_into().expect("16 字节")).into_uuid()
    )))
}

const MAGIC_TABLE: &[(&[u8], &str)] = &[
    (b"\x7fELF", "ELF 可执行/共享库（.so、可执行文件）"),
    (
        b"MZ",
        "PE/DEX 之类以 MZ 开头的容器（apk 里的 classes.dex 也常以 dex\\n 打头）",
    ),
    (b"dex\n", "Android DEX"),
    (b"\xca\xfe\xba\xbe", "Mach-O fat"),
    (b"\xcf\xfa\xed\xfe", "Mach-O 64 位"),
    (b"\x89PNG", "PNG"),
    (b"\xff\xd8\xff", "JPEG"),
    (b"GIF8", "GIF"),
    (b"%PDF", "PDF"),
    (b"PK\x03\x04", "ZIP 容器（APK/AAR/DOCX/JAR 都是它）"),
    (b"\x1f\x8b", "gzip"),
    (b"BZh", "bzip2"),
    (b"7z\xbc\xaf", "7z"),
    (b"Rar!", "RAR"),
    (b"\x5d\x00\x00", "lzma"),
    (b"\x7b", "以 { 开头：大概率是 JSON 文本"),
    (b"WASM", "WASM 模块（旧版魔数是 \\0asm）"),
    (b"SQLite format 3", "SQLite 数据库"),
    (b"OggS", "Ogg"),
    (b"ftyp", "MP4/MOV 容器（偏移 4 处）"),
];

fn detect_file_type(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    if data.bytes.is_empty() {
        return Err(fail("没有数据可识别（输入框是空的）"));
    }
    let head = &data.bytes[..data.bytes.len().min(32)];
    let mut hits: Vec<String> = Vec::new();
    for (magic, label) in MAGIC_TABLE {
        if head.starts_with(magic) {
            hits.push((*label).to_string());
        }
        if magic.len() <= head.len() && head.windows(magic.len()).take(4).any(|w| w == *magic) {
            hits.push(format!("{label}（出现在前 32 字节内）"));
        }
    }
    let printable = head
        .iter()
        .filter(|b| b.is_ascii_graphic() || matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        .count();
    if printable == head.len() {
        hits.push("前 32 字节全部可打印：像文本".to_string());
    }
    let mut out = String::new();
    out.push_str(&format!(
        "前 {} 字节：{}\n",
        head.len(),
        hex_encode(&head[..head.len().min(16)], false, " ")
    ));
    if hits.is_empty() {
        out.push_str("认不出来（不在内置魔数表里）");
    } else {
        for hit in hits {
            out.push_str(&format!("· {hit}\n"));
        }
    }
    Ok(ChefData::from_text(out.trim_end_matches('\n')))
}

pub const OPS: &[super::Op] = &[
    op(
        "jsonBeautify",
        "format",
        &[number("indent", 2)],
        json_beautify,
    ),
    op("jsonMinify", "format", &[], json_minify),
    op(
        "jwtDecode",
        "format",
        &[
            number("indent", 2),
            bool_arg("humanReadableDates", true),
            text("secret", ""),
        ],
        jwt_decode,
    ),
    op(
        "hexDump",
        "format",
        &[
            choice("style", "xxd", &["xxd", "hexdump", "plain"]),
            number("bytesPerLine", 16),
        ],
        hex_dump,
    ),
    op(
        "protobufDecode",
        "format",
        &[bool_arg("preferText", true)],
        protobuf_decode,
    ),
    op("derDecode", "format", &[], der_decode),
    op("parseUuid", "format", &[], parse_uuid),
    op("detectFileType", "format", &[], detect_file_type),
];

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
