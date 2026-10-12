//! 摘要与校验和：MD5 / SHA-1 / SHA-2 / SHA-3 / Keccak / BLAKE2 / BLAKE3 /
//! RIPEMD-160 / SM3 / NTLM，以及 CRC、Adler-32、Fletcher。
//!
//! 为什么要这么多种哈希：逆向时靠哈希长度与字符集反推「这段签名用了什么算法」
//! 是最快的一条线索，清单里缺哪个就等于逼用户去装另一个工具。

use digest::Digest;

use crate::services::chef::data::{Args, ChefData, ChefResult, fail, hex_encode};
use crate::services::chef::spec::{choice, op};

/// 摘要输出的公共尾巴：按 `uppercase` 决定十六进制大小写（默认小写，
/// 因为绝大多数接口与日志里的哈希是小写）
fn render_digest(bytes: &[u8], args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::from_text(&hex_encode(
        bytes,
        args.bool_of("uppercase"),
        "",
    )))
}

fn digest_with<D: Digest>(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let mut hasher = D::new();
    hasher.update(&data.bytes);
    let outcome = hasher.finalize();
    render_digest(&outcome, args)
}

/// 摘要类算法只差在 `Digest` 实现上，用宏把「包一层函数」这件事压成一行；
/// 注册表里的表项仍然显式写 id，方便按 id 搜索到实现。
macro_rules! define_digest_fns {
    ($($name:ident => $algo:path),* $(,)?) => {
        $(
            fn $name(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
                digest_with::<$algo>(data, args)
            }
        )*
    };
}

define_digest_fns! {
    h_md4 => md4::Md4,
    h_md5 => md5::Md5,
    h_sha1 => sha1::Sha1,
    h_sha224 => sha2::Sha224,
    h_sha256 => sha2::Sha256,
    h_sha384 => sha2::Sha384,
    h_sha512 => sha2::Sha512,
    h_sha3_256 => sha3::Sha3_256,
    h_sha3_512 => sha3::Sha3_512,
    h_keccak_256 => sha3::Keccak256,
    h_keccak_512 => sha3::Keccak512,
    h_blake2b => blake2::Blake2b512,
    h_blake2s => blake2::Blake2s256,
    h_ripemd160 => ripemd::Ripemd160,
    h_sm3 => sm3::Sm3,
}

/// 所有摘要算法共用这一张参数表（只有大小写一项）
const UPPERCASE_ONLY: &[crate::services::chef::spec::ArgSpec] =
    &[crate::services::chef::spec::bool_arg("uppercase", false)];

/// BLAKE3 不挂 digest 0.10 的 trait（自带 API），单独包一层
fn h_blake3(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let outcome = blake3::hash(&data.bytes);
    let hex = outcome.to_hex();
    render_digest(
        &hex::decode(hex.as_str()).map_err(|e| fail(e.to_string()))?,
        args,
    )
}

/// NTLM / NT hash：UTF-16LE 明文取 MD5。安卓与 Windows 侧凭证分析里最常见的一类
fn nt_hash(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let text = data.text()?;
    let utf16: Vec<u8> = text
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    digest_with::<md4::Md4>(&ChefData::new(utf16), args)
}

fn h_crc32(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&data.bytes);
    let value = hasher.finalize();
    Ok(ChefData::from_text(&format_value(
        value as u64,
        32,
        args.bool_of("uppercase"),
    )))
}

fn format_value(value: u64, width: u32, uppercase: bool) -> String {
    let hex = format!("{value:0width$x}", width = (width / 4) as usize);
    if uppercase { hex.to_uppercase() } else { hex }
}

fn is_hex_upper(text: &str) -> bool {
    text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// 反射位（CRC 目录里 refin/refout 的口径）
fn reflect(value: u64, width: u32) -> u64 {
    let mut out = 0u64;
    for bit in 0..width {
        if (value >> bit) & 1 == 1 {
            out |= 1 << (width - 1 - bit);
        }
    }
    out
}

/// 目录法 CRC 计算（宽度 8–64，支持反射与非反射）
pub fn crc_compute(
    bytes: &[u8],
    width: u32,
    poly: u64,
    init: u64,
    refin: bool,
    refout: bool,
    xorout: u64,
) -> u64 {
    let mask = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let top = 1u64 << (width - 1);
    let mut crc = init & mask;
    for byte in bytes {
        if width == 8 {
            crc ^= *byte as u64;
        } else {
            let fed = if refin {
                reflect(*byte as u64, 8)
            } else {
                *byte as u64
            };
            crc ^= fed << (width - 8);
        }
        crc &= mask;
        for _ in 0..8 {
            crc = if crc & top != 0 {
                ((crc << 1) ^ poly) & mask
            } else {
                (crc << 1) & mask
            };
        }
    }
    if refout {
        crc = reflect(crc, width);
    }
    (crc ^ xorout) & mask
}

/// `params.h` 里那套 RevEng 目录常量。键与算法清单的下拉值一一对应。
type CrcParams = (u32, u64, u64, bool, bool, u64);

const CRC_PRESETS: &[(&str, CrcParams)] = &[
    ("crc8", (8, 0x07, 0x00, false, false, 0x00)),
    ("crc16-ibm", (16, 0x8005, 0x0000, true, true, 0x0000)),
    ("crc16-ccitt", (16, 0x1021, 0xFFFF, false, false, 0x0000)),
    ("crc16-modbus", (16, 0x8005, 0xFFFF, true, true, 0x0000)),
    (
        "crc32",
        (32, 0x04C1_1DB7, 0xFFFF_FFFF, true, true, 0xFFFF_FFFF),
    ),
    (
        "crc32b",
        (32, 0x04C1_1DB7, 0xFFFF_FFFF, false, false, 0xFFFF_FFFF),
    ),
    (
        "crc32c",
        (32, 0x1EDC_6F41, 0xFFFF_FFFF, true, true, 0xFFFF_FFFF),
    ),
    (
        "crc64-ecma",
        (64, 0x42F0_E1EB_A9EA_3693, 0, false, false, 0),
    ),
    (
        "crc64-xz",
        (
            64,
            0x42F0_E1EB_A9EA_3693,
            0xFFFF_FFFF_FFFF_FFFF,
            true,
            true,
            0xFFFF_FFFF_FFFF_FFFF,
        ),
    ),
];

const CRC_OPTIONS: &[&str] = &[
    "crc8",
    "crc16-ibm",
    "crc16-ccitt",
    "crc16-modbus",
    "crc32",
    "crc32b",
    "crc32c",
    "crc64-ecma",
    "crc64-xz",
    "custom",
];

fn parse_hex_u64(text: &str, what: &str) -> ChefResult<u64> {
    let trimmed = text.trim().trim_start_matches("0x").replace(' ', "");
    if trimmed.is_empty() {
        return Err(fail(format!("{what}不能为空")));
    }
    u64::from_str_radix(&trimmed, 16).map_err(|_| fail(format!("{what}「{text}」不是十六进制")))
}

fn h_crc(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let preset = args.choice_in("preset", "crc32", CRC_OPTIONS)?;
    let (width, poly, init, refin, refout, xorout) = if preset == "custom" {
        let width = args.number("width", 32)? as u32;
        if !(8..=64).contains(&width) {
            return Err(fail("自定义 CRC 的宽度要在 8~64 之间"));
        }
        (
            width,
            parse_hex_u64(&args.text("polynomial", ""), "多项式")?,
            parse_hex_u64(&args.text("initialValue", ""), "初始值")?,
            args.bool_of("reflectIn"),
            args.bool_of("reflectOut"),
            parse_hex_u64(&args.text("xorOut", ""), "输出亦或")?,
        )
    } else {
        CRC_PRESETS
            .iter()
            .find(|(name, _)| *name == preset)
            .ok_or_else(|| fail(format!("未知的 CRC 预设「{preset}」")))?
            .1
    };
    let value = crc_compute(&data.bytes, width, poly, init, refin, refout, xorout);
    Ok(ChefData::from_text(&format_value(
        value,
        width,
        args.bool_of("uppercase"),
    )))
}

fn h_adler32(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    // 直接按 RFC 1950 定义算：16 位部分和模 65521，两半拼成 32 位
    let (mut s1, mut s2) = (1u32, 0u32);
    for byte in &data.bytes {
        s1 = (s1 + *byte as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    Ok(ChefData::from_text(&format_value(
        ((s2 as u64) << 16) | s1 as u64,
        32,
        args.bool_of("uppercase"),
    )))
}

fn h_fletcher(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let variant = args.choice_in("variant", "fletcher16", &["fletcher16", "fletcher32"])?;
    let bytes = &data.bytes;
    let value = match variant.as_str() {
        "fletcher16" => {
            let (mut s1, mut s2) = (0u16, 0u16);
            for byte in bytes {
                s1 = (s1 + *byte as u16) % 255;
                s2 = (s2 + s1) % 255;
            }
            ((s2 as u64) << 8) | s1 as u64
        }
        // Fletcher-32 按小端 16 位字取数，奇数长度尾部补一个 0 字节（RFC 1903 口径）
        _ => {
            let (mut s1, mut s2) = (0u32, 0u32);
            let mut index = 0;
            while index < bytes.len() {
                let low = bytes[index] as u32;
                let high = bytes.get(index + 1).copied().unwrap_or(0) as u32;
                let word = low | (high << 8);
                s1 = (s1 + word) % 65535;
                s2 = (s2 + s1) % 65535;
                index += 2;
            }
            ((s2 as u64) << 16) | s1 as u64
        }
    };
    let width = if variant == "fletcher16" { 16 } else { 32 };
    Ok(ChefData::from_text(&format_value(
        value,
        width,
        args.bool_of("uppercase"),
    )))
}

/// 只看样子猜哈希类型：长度 + 字符集 + 前缀三条线索足够把候选缩到几个。
/// 明确说「这是候选，不是结论」，避免用户把猜的东西当证据。
fn identify_hash(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let text = data.text()?.trim().to_string();
    if text.is_empty() {
        return Err(fail("要识别的哈希是空的"));
    }
    let lower = text.to_lowercase();
    let is_hex = lower.chars().all(|c| c.is_ascii_hexdigit());
    let is_b64 = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'));
    let mut candidates: Vec<&str> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    if is_hex {
        let by_len: &[&str] = match lower.len() {
            8 => &["CRC-32", "Adler-32", "Fletcher-32", "Sum-32"],
            16 => &["CRC-64", "MD5 原始值截断?", "Snefru-128", "RIPEMD-128"],
            32 => &[
                "MD5",
                "MD4",
                "RIPEMD-128",
                "NTLM (NT hash)",
                "Snefru-128",
                "MySQL老密码(3.23前)",
            ],
            40 => &["SHA-1", "HAVAL-160?", "MySQL 4.1+ 密码", "RIPEMD-160"],
            56 => &["SHA-224", "CRC-256?"],
            64 => &[
                "SHA-256",
                "RIPEMD-256",
                "Keccak-256",
                "BLAKE-256",
                "SM3",
                "HAVAL-256",
                "Snefru-256",
                "GOST R 34.11-94",
            ],
            96 => &["SHA-384", "RIPEMD-384"],
            128 => &["SHA-512", "Keccak-512", "SHA-3-512", "RIPEMD-512"],
            _ => &[],
        };
        candidates.extend(by_len.iter());
        if lower.len() == 32 && text.chars().any(|c| c.is_ascii_uppercase()) {
            notes.push("全大写十六进制：更像 MD5/NTLM 的输出习惯".to_string());
        }
    } else if is_b64 && !text.contains(' ') {
        let bare = text.trim_end_matches('=');
        match bare.len() {
            22 => candidates.push("128 位原始值 Base64url（MD5/SHA-1 截断）"),
            27 | 28 => candidates.push("SHA-1 原始值 Base64（20 字节）"),
            43 | 44 => candidates.push("SHA-256 原始值 Base64（32 字节）"),
            86 | 88 => candidates.push("SHA-512 原始值 Base64（64 字节）"),
            _ => {}
        }
    }

    if text.starts_with("$1$") {
        candidates.clear();
        candidates.push("MD5crypt")
    } else if text.starts_with("$2a$") || text.starts_with("$2b$") || text.starts_with("$2y$") {
        candidates.clear();
        candidates.push("bcrypt")
    } else if text.starts_with("$5$") {
        candidates.clear();
        candidates.push("sha256crypt")
    } else if text.starts_with("$6$") {
        candidates.clear();
        candidates.push("sha512crypt")
    } else if text.starts_with("$argon2") {
        candidates.clear();
        candidates.push("Argon2")
    } else if text.starts_with("$pbkdf2-sha256$") {
        candidates.clear();
        candidates.push("PBKDF2-HMAC-SHA256")
    } else if text.chars().count() == 13 && text.starts_with('_') {
        candidates.clear();
        candidates.push("BSDi md5crypt 旧式")
    } else if text.starts_with('*') && text.len() == 41 && is_hex_upper(&text) {
        candidates.push("MySQL 4.1+ 密码哈希（SHA-1 双轮）")
    }

    if candidates.is_empty() {
        return Ok(ChefData::from_text(&format!(
            "认不出来（{len} 个字符）。\n提示：候选算法靠长度与字符集判断，这段两者都不匹配常见格式。",
            len = text.chars().count()
        )));
    }
    let mut out = String::from("可能的算法（按常见程度，不是结论）：\n");
    for candidate in &candidates {
        out.push_str(&format!("· {candidate}\n"));
    }
    out.truncate(out.trim_end_matches('\n').len());
    for note in notes {
        out.push_str(&format!("\n注意：{note}"));
    }
    Ok(ChefData::from_text(&out))
}

pub const OPS: &[super::Op] = &[
    op("md4", "digest", UPPERCASE_ONLY, h_md4),
    op("md5", "digest", UPPERCASE_ONLY, h_md5),
    op("sha1", "digest", UPPERCASE_ONLY, h_sha1),
    op("sha224", "digest", UPPERCASE_ONLY, h_sha224),
    op("sha256", "digest", UPPERCASE_ONLY, h_sha256),
    op("sha384", "digest", UPPERCASE_ONLY, h_sha384),
    op("sha512", "digest", UPPERCASE_ONLY, h_sha512),
    op("sha3_256", "digest", UPPERCASE_ONLY, h_sha3_256),
    op("sha3_512", "digest", UPPERCASE_ONLY, h_sha3_512),
    op("keccak_256", "digest", UPPERCASE_ONLY, h_keccak_256),
    op("keccak_512", "digest", UPPERCASE_ONLY, h_keccak_512),
    op("blake2b", "digest", UPPERCASE_ONLY, h_blake2b),
    op("blake2s", "digest", UPPERCASE_ONLY, h_blake2s),
    op("blake3", "digest", UPPERCASE_ONLY, h_blake3),
    op("ripemd160", "digest", UPPERCASE_ONLY, h_ripemd160),
    op("sm3", "digest", UPPERCASE_ONLY, h_sm3),
    op("ntHash", "digest", UPPERCASE_ONLY, nt_hash),
    op("identifyHash", "digest", &[], identify_hash),
    op("crc32", "checksum", UPPERCASE_ONLY, h_crc32),
    op(
        "crc",
        "checksum",
        &[
            choice("preset", "crc32", CRC_OPTIONS),
            crate::services::chef::spec::number("width", 32),
            crate::services::chef::spec::text("polynomial", "04C11DB7"),
            crate::services::chef::spec::text("initialValue", "FFFFFFFF"),
            crate::services::chef::spec::bool_arg("reflectIn", true),
            crate::services::chef::spec::bool_arg("reflectOut", true),
            crate::services::chef::spec::text("xorOut", "FFFFFFFF"),
            crate::services::chef::spec::bool_arg("uppercase", false),
        ],
        h_crc,
    ),
    op("adler32", "checksum", UPPERCASE_ONLY, h_adler32),
    op(
        "fletcher",
        "checksum",
        &[
            choice("variant", "fletcher16", &["fletcher16", "fletcher32"]),
            crate::services::chef::spec::bool_arg("uppercase", false),
        ],
        h_fletcher,
    ),
];

#[cfg(test)]
mod tests {
    use crate::services::chef::testing::{expect_error, run_op_text};

    fn hex(id: &str, input: &str, overrides: &[(&str, &str)]) -> String {
        run_op_text(id, input, overrides)
    }

    #[test]
    fn md5_sha_vectors() {
        // 与 RFC / OpenSSL 参考值一致（"abc"、"hello"、整句）
        assert_eq!(hex("md5", "abc", &[]), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex("md5", "", &[]), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            hex("md5", "The quick brown fox jumps over the lazy dog", &[]),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(
            hex("sha1", "abc", &[]),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex("sha224", "abc", &[]),
            "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
        );
        assert_eq!(
            hex("sha256", "abc", &[]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex("sha384", "abc", &[]),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex("sha512", "abc", &[]),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex("sha256", "abc", &[("uppercase", "true")]),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }

    #[test]
    fn sha3_is_not_keccak() {
        // 两者常被混用，必须能区分：同一句 "abc" 的填充规则不同
        assert_eq!(
            hex("sha3_256", "abc", &[]),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        assert_eq!(
            hex("keccak_256", "abc", &[]),
            "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
        );
        assert_eq!(
            hex("keccak_256", "", &[]),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert_eq!(
            hex("sha3_512", "abc", &[]),
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0"
        );
        assert_ne!(hex("sha3_256", "abc", &[]), hex("keccak_256", "abc", &[]));
    }

    #[test]
    fn blake_and_ripemd_and_sm3() {
        assert_eq!(
            hex("blake2b", "abc", &[]),
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
        assert_eq!(
            hex("blake2s", "abc", &[]),
            "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"
        );
        assert_eq!(
            hex("blake3", "", &[]),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            hex("ripemd160", "abc", &[]),
            "8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
        );
        // GB/T 32905 官方示例 1
        assert_eq!(
            hex("sm3", "abc", &[]),
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
        );
        // 官方示例 2：64 字节 "abcd"*16
        let abcd = "abcd".repeat(16);
        assert_eq!(
            hex("sm3", &abcd, &[]),
            "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732"
        );
    }

    #[test]
    fn md4_vectors() {
        // RFC 1320 附录里的 MD4 测试串
        assert_eq!(hex("md4", "", &[]), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex("md4", "abc", &[]), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(
            hex("md4", "Message Digest", &[]),
            "37631f447b070e49764315ece76e396a"
        );
    }

    #[test]
    fn ntlm_vectors() {
        // NT hash = MD4(UTF-16LE(口令))：空口令与 "password" 都是业界公开值
        assert_eq!(hex("ntHash", "", &[]), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(
            hex("ntHash", "password", &[]),
            "8846f7eaee8fb117ad06bdd830b7586c"
        );
        // 中文口令走 UTF-16LE 再取 MD4。与「MD4(utf16le)」显式对齐一次，
        // 保证 ntHash 不是另写了一遍拼接逻辑
        let utf16: Vec<u8> = "中文"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(
            run_op_text("ntHash", "中文", &[]),
            crate::services::chef::testing::run_op_bytes("md4", &utf16, &[]).text_lossy()
        );
    }

    #[test]
    fn checksum_vectors() {
        assert_eq!(hex("crc32", "abc", &[]), "352441c2");
        assert_eq!(hex("crc32", "", &[]), "00000000");
        assert_eq!(hex("adler32", "abc", &[]), "024d0127");
        assert_eq!(hex("fletcher", "abc", &[]), "4c27");
        assert_eq!(
            hex("fletcher", "abc", &[("variant", "fletcher32")]),
            "c52562c4"
        );
        // RevEng 目录的 check 值（"123456789"）
        for (preset, expected) in [
            ("crc8", "f4"),
            ("crc16-ibm", "bb3d"),
            ("crc16-ccitt", "29b1"),
            ("crc16-modbus", "4b37"),
            ("crc32", "cbf43926"),
            ("crc32b", "fc891918"),
            ("crc32c", "e3069283"),
            ("crc64-ecma", "6c40df5f0b497347"),
            ("crc64-xz", "995dc9bbdf1939fa"),
        ] {
            assert_eq!(
                hex("crc", "123456789", &[("preset", preset)]),
                expected,
                "预设 {preset}"
            );
        }
        // custom 参数必须能复现 CRC-32（否则自定义那一档没有意义）
        assert_eq!(
            hex(
                "crc",
                "123456789",
                &[
                    ("preset", "custom"),
                    ("width", "32"),
                    ("polynomial", "04C11DB7"),
                    ("initialValue", "FFFFFFFF"),
                    ("reflectIn", "true"),
                    ("reflectOut", "true"),
                    ("xorOut", "FFFFFFFF")
                ]
            ),
            "cbf43926"
        );
        let message = expect_error("crc", "123456789", &[("preset", "custom"), ("width", "7")]);
        assert!(message.contains("8~64"), "{message}");
    }

    #[test]
    fn identify_hash_candidates() {
        let md5_like = hex("identifyHash", "900150983cd24fb0d6963f7d28e17f72", &[]);
        assert!(md5_like.contains("MD5"), "{md5_like}");
        assert!(md5_like.contains("NTLM"), "{md5_like}");
        let sha1_like = hex(
            "identifyHash",
            "a9993e364706816aba3e25717850c26c9cd0d89d",
            &[],
        );
        assert!(sha1_like.contains("SHA-1"), "{sha1_like}");
        let bcrypt = hex("identifyHash", "$2b$12$abc", &[]);
        assert!(bcrypt.contains("bcrypt"), "{bcrypt}");
        let unknown = hex("identifyHash", "这不是哈希", &[]);
        assert!(unknown.contains("认不出来"), "{unknown}");
    }

    #[test]
    fn empty_input_hashes_are_not_empty() {
        // 空输入的摘要必须仍是摘要值本身（曾被当成「这步没输出」而显示空白）
        assert_eq!(hex("sha256", "", &[]).len(), 64);
    }
}
