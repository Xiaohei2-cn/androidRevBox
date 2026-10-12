//! 消息认证与密钥派生：HMAC、PBKDF2、HKDF、bcrypt。
//!
//! 这一组是接口签名逆向的主力：App 里 `sign = HMAC-SHA256(appSecret, ts + body)`
//! 这类写法太多，界面能选摘要算法与密钥格式，就能一把把它试出来。

use digest::KeyInit;
use hmac::{Hmac, Mac};

use crate::services::chef::data::{
    Args, ChefData, ChefResult, emit_by_format, fail, hex_encode, parse_param_bytes, read_by_format,
};
use crate::services::chef::spec::{bool_arg, choice, number, op, text};

/// HMAC 支持的摘要：id 与「摘要」那一组算法的 id 同名，界面上的选项能互相对照
const HMAC_ALGORITHMS: &[&str] = &[
    "md5",
    "sha1",
    "sha256",
    "sha384",
    "sha512",
    "sha3_256",
    "sha3_512",
    "ripemd160",
    "sm3",
];
const KEY_FORMATS: &[&str] = &["utf8", "hex", "base64"];
const OUTPUT_FORMATS: &[&str] = &["hex", "base64", "decimal", "utf8"];

/// 派生长度上限：4 KB 够任何 KDF 用法，再大基本是填错了单位
const MAX_DERIVED_KEY: u64 = 4096;
/// 迭代上限：超了就是几分钟级别的等待，明确拒绝比让界面像卡死好
const MAX_ITERATIONS: u64 = 2_000_000;

fn emit_digest(bytes: &[u8], args: &Args) -> ChefResult<ChefData> {
    let format = args.choice_in("output", "hex", OUTPUT_FORMATS)?;
    if format == "hex" && args.bool_of("uppercase") {
        return Ok(ChefData::from_text(&hex_encode(bytes, true, "")));
    }
    emit_by_format(bytes.to_vec(), &format)
}

fn hmac_with<M: Mac + KeyInit>(data: &ChefData, args: &Args, key: &[u8]) -> ChefResult<ChefData> {
    let mut mac = <M as KeyInit>::new_from_slice(key)
        .map_err(|_| fail("HMAC 不接受这个长度的密钥（内部：Mac::new_from_slice 拒绝）"))?;
    mac.update(&data.bytes);
    let tag = mac.finalize().into_bytes();
    emit_digest(tag.as_ref(), args)
}

macro_rules! hmac_alias {
    ($name:ident, $algo:path) => {
        type $name = Hmac<$algo>;
    };
}

hmac_alias!(HmacMd5, md5::Md5);
hmac_alias!(HmacSha1, sha1::Sha1);
hmac_alias!(HmacSha256, sha2::Sha256);
hmac_alias!(HmacSha384, sha2::Sha384);
hmac_alias!(HmacSha512, sha2::Sha512);
hmac_alias!(HmacSha3_256, sha3::Sha3_256);
hmac_alias!(HmacSha3_512, sha3::Sha3_512);
hmac_alias!(HmacRipemd160, ripemd::Ripemd160);
hmac_alias!(HmacSm3, sm3::Sm3);

fn hmac(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let algorithm = args.choice_in("algorithm", "sha256", HMAC_ALGORITHMS)?;
    let key_format = args.choice_in("keyFormat", "utf8", KEY_FORMATS)?;
    let raw_key = args.text("key", "");
    if raw_key.is_empty() {
        // 空密钥在密码学上合法，但在这个界面里它 100% 是「忘了填」，
        // 而拿空密钥算出来的签名跟谁都对不上——必须直说，不能默默给个结果
        return Err(fail(
            "还差一步：HMAC 的密钥是空的。先填 key（可用 utf8 / hex / base64 三种写法）",
        ));
    }
    let key = parse_param_bytes(&raw_key, &key_format)?;
    match algorithm.as_str() {
        "md5" => hmac_with::<HmacMd5>(data, args, &key),
        "sha1" => hmac_with::<HmacSha1>(data, args, &key),
        "sha256" => hmac_with::<HmacSha256>(data, args, &key),
        "sha384" => hmac_with::<HmacSha384>(data, args, &key),
        "sha512" => hmac_with::<HmacSha512>(data, args, &key),
        "sha3_256" => hmac_with::<HmacSha3_256>(data, args, &key),
        "sha3_512" => hmac_with::<HmacSha3_512>(data, args, &key),
        "ripemd160" => hmac_with::<HmacRipemd160>(data, args, &key),
        "sm3" => hmac_with::<HmacSm3>(data, args, &key),
        other => Err(fail(format!("不支持的 HMAC 摘要「{other}」"))),
    }
}

fn guard_iterations(args: &Args) -> ChefResult<u64> {
    let iterations = args.number("iterations", 100_000)?;
    if iterations == 0 {
        return Err(fail("迭代次数不能是 0"));
    }
    if iterations > MAX_ITERATIONS {
        return Err(fail(format!(
            "迭代次数 {iterations} 超过上限 {MAX_ITERATIONS}：一次派生要等到分钟级。把迭代数调小再来"
        )));
    }
    Ok(iterations)
}

fn guard_length(args: &Args, key: &str, default: u64) -> ChefResult<usize> {
    let length = args.number(key, default)?;
    if length == 0 || length > MAX_DERIVED_KEY {
        return Err(fail(format!(
            "{key} 要在 1–{MAX_DERIVED_KEY} 字节之间（收到 {length}）"
        )));
    }
    Ok(length as usize)
}

fn pbkdf2(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let algorithm = args.choice_in("algorithm", "sha256", &["sha1", "sha256", "sha512"])?;
    let iterations = guard_iterations(args)?;
    let length = guard_length(args, "keyLength", 32)?;
    let salt = parse_param_bytes(
        &args.text("salt", ""),
        &args.choice_in("saltFormat", "utf8", KEY_FORMATS)?,
    )?;
    // 口令从流水线取：这样「先 Base64 解码再派生」这类链子才成立
    let password = read_by_format(
        data,
        &args.choice_in("passwordFormat", "utf8", KEY_FORMATS)?,
    )?;
    let mut out = vec![0u8; length];
    match algorithm.as_str() {
        "sha1" => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&password, &salt, iterations as u32, &mut out),
        "sha256" => {
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(&password, &salt, iterations as u32, &mut out)
        }
        "sha512" => {
            pbkdf2::pbkdf2_hmac::<sha2::Sha512>(&password, &salt, iterations as u32, &mut out)
        }
        other => return Err(fail(format!("不支持的 PBKDF2 摘要「{other}」"))),
    }
    emit_digest(&out, args)
}

fn hkdf(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let algorithm = args.choice_in("algorithm", "sha256", &["sha256", "sha384", "sha512"])?;
    let length = guard_length(args, "keyLength", 32)?;
    // salt/info 按各自格式解释：RFC 5869 的示例是二进制，一律按 UTF-8 读必错
    let salt = parse_param_bytes(
        &args.text("salt", ""),
        &args.choice_in("saltFormat", "utf8", KEY_FORMATS)?,
    )?;
    let info = parse_param_bytes(
        &args.text("info", ""),
        &args.choice_in("infoFormat", "utf8", KEY_FORMATS)?,
    )?;
    let ikm = read_by_format(data, &args.choice_in("secretFormat", "utf8", KEY_FORMATS)?)?;
    let mut out = vec![0u8; length];
    let result: ChefResult<()> = match algorithm.as_str() {
        "sha256" => {
            let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
            hk.expand(&info, &mut out)
                .map_err(|e| fail(format!("HKDF 扩展失败：{e}")))
        }
        "sha384" => {
            let hk = hkdf::Hkdf::<sha2::Sha384>::new(Some(&salt), &ikm);
            hk.expand(&info, &mut out)
                .map_err(|e| fail(format!("HKDF 扩展失败：{e}")))
        }
        "sha512" => {
            let hk = hkdf::Hkdf::<sha2::Sha512>::new(Some(&salt), &ikm);
            hk.expand(&info, &mut out)
                .map_err(|e| fail(format!("HKDF 扩展失败：{e}")))
        }
        other => Err(fail(format!("不支持的 HKDF 摘要「{other}」"))),
    };
    result?;
    emit_digest(&out, args)
}

fn bcrypt_version(prefix: &str) -> ChefResult<bcrypt::Version> {
    match prefix {
        "$2a$" => Ok(bcrypt::Version::TwoA),
        "$2b$" => Ok(bcrypt::Version::TwoB),
        "$2y$" => Ok(bcrypt::Version::TwoY),
        "$2x$" => Ok(bcrypt::Version::TwoX),
        other => Err(fail(format!("未知的 bcrypt 版本前缀「{other}」"))),
    }
}

fn bcrypt_hash(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let cost = args.number("cost", 12)?;
    if !(4..=15).contains(&cost) {
        return Err(fail(format!(
            "bcrypt 的 cost 要在 4–15 之间（收到 {cost}）"
        )));
    }
    let password = data.text()?;
    if password.len() > 72 {
        // bcrypt 会静悄悄地把 72 字节之后丢掉：不说出来就会被当成“这个哈希更强”
        return Err(fail(format!(
            "口令 {len} 字节，而 bcrypt 只取前 72 字节（后面那 {extra} 字节根本不参与计算）。要么换算法，要么把口令截到 72 字节",
            len = password.len(),
            extra = password.len() - 72
        )));
    }
    let version = bcrypt_version(&args.choice("version", "$2b$"))?;
    let parts = bcrypt::hash_with_result(password.as_bytes(), cost as u32)
        .map_err(|e| fail(format!("bcrypt 计算失败：{e}")))?;
    Ok(ChefData::from_text(&parts.format_for_version(version)))
}

fn bcrypt_verify(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let hash = args.text("hash", "");
    if hash.is_empty() {
        return Err(fail(
            "还差一步：把要校验的 bcrypt 哈希填进 hash（形如 $2b$12$…）",
        ));
    }
    let password = data.text()?;
    let matched = bcrypt::verify(password.as_bytes(), &hash)
        .map_err(|e| fail(format!("给出的哈希读不动，不是合法 bcrypt：{e}")))?;
    Ok(ChefData::from_text(if matched {
        "匹配：这个口令与给出的哈希是同一对"
    } else {
        "不匹配：口令与哈希不是同一对"
    }))
}

pub const OPS: &[super::Op] = &[
    op(
        "hmac",
        "kdf",
        &[
            choice("algorithm", "sha256", HMAC_ALGORITHMS),
            text("key", ""),
            choice("keyFormat", "utf8", KEY_FORMATS),
            choice("output", "hex", OUTPUT_FORMATS),
            bool_arg("uppercase", false),
        ],
        hmac,
    ),
    op(
        "pbkdf2",
        "kdf",
        &[
            choice("algorithm", "sha256", &["sha1", "sha256", "sha512"]),
            choice("passwordFormat", "utf8", KEY_FORMATS),
            text("salt", ""),
            choice("saltFormat", "utf8", KEY_FORMATS),
            number("iterations", 100_000),
            number("keyLength", 32),
            choice("output", "hex", OUTPUT_FORMATS),
            bool_arg("uppercase", false),
        ],
        pbkdf2,
    ),
    op(
        "hkdf",
        "kdf",
        &[
            choice("algorithm", "sha256", &["sha256", "sha384", "sha512"]),
            choice("secretFormat", "utf8", KEY_FORMATS),
            text("salt", ""),
            choice("saltFormat", "utf8", KEY_FORMATS),
            text("info", ""),
            choice("infoFormat", "utf8", KEY_FORMATS),
            number("keyLength", 32),
            choice("output", "hex", OUTPUT_FORMATS),
            bool_arg("uppercase", false),
        ],
        hkdf,
    ),
    op(
        "bcryptHash",
        "kdf",
        &[
            number("cost", 12),
            choice("version", "$2b$", &["$2a$", "$2b$", "$2y$", "$2x$"]),
        ],
        bcrypt_hash,
    ),
    op("bcryptVerify", "kdf", &[text("hash", "")], bcrypt_verify),
];

#[cfg(test)]
mod tests {
    use crate::services::chef::testing::{expect_error, run_op_bytes, run_op_text};

    fn hmac_of(input: &str, overrides: &[(&str, &str)]) -> String {
        run_op_text("hmac", input, overrides)
    }

    #[test]
    fn hmac_rfc2202_and_4231_vectors() {
        // RFC 2202 / RFC 4231 的第二组：key="Jefe"
        let data = "what do ya want for nothing?";
        let key = &[("key", "Jefe")][..];
        assert_eq!(
            hmac_of(data, key),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hmac_of(data, &[("key", "Jefe"), ("algorithm", "sha1")]),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        assert_eq!(
            hmac_of(data, &[("key", "Jefe"), ("algorithm", "md5")]),
            "750c783e6ab0b503eaa86e310a5db738"
        );
        assert_eq!(
            hmac_of(data, &[("key", "Jefe"), ("algorithm", "sha512")]),
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        );
        // key="key" / data="abc" 这一组用另一套实现（Python hmac）交叉核对过
        assert_eq!(
            hmac_of("abc", &[("key", "key")]),
            "9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab"
        );
        assert_eq!(
            hmac_of("abc", &[("key", "key"), ("algorithm", "sha1")]),
            "4fd0b215276ef12f2b3e4c8ecac2811498b656fc"
        );
        assert_eq!(
            hmac_of("abc", &[("key", "key"), ("algorithm", "md5")]),
            "d2fe98063f876b03193afb49b4979591"
        );
        assert_eq!(
            hmac_of("abc", &[("key", "key"), ("algorithm", "sha3_256")]),
            "09b6dbab8d11795ca7c8d82f1cf91682013c7cb980abbb25473be4ae7f7b5683"
        );
    }

    #[test]
    fn sm3_hmac_works_and_differs() {
        // HMAC-SM3 是国密接口里最常见的签名写法；单独证明它真的算出了 32 字节摘要
        let digest = hmac_of("abc", &[("key", "key"), ("algorithm", "sm3")]);
        assert_eq!(digest.len(), 64, "{digest}");
        assert_ne!(
            digest,
            hmac_of("abc", &[("key", "key"), ("algorithm", "sha256")])
        );
    }

    #[test]
    fn key_format_selects_how_key_is_read() {
        // "key" 的三种写法必须是同一份字节：hex/base64 选错就永远对不上签名
        for key in ["key", "6b6579", "a2V5"] {
            let (format, _) = match key {
                "key" => ("utf8", 0),
                "6b6579" => ("hex", 1),
                _ => ("base64", 2),
            };
            assert_eq!(
                hmac_of("abc", &[("key", key), ("keyFormat", format)]),
                "9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab",
                "密钥写法 {key}（{format}）"
            );
        }
        // 反过来：把 hex 串当 UTF-8 密钥用，结果一定不同
        assert_ne!(
            hmac_of("abc", &[("key", "6b6579")]),
            hmac_of("abc", &[("key", "6b6579"), ("keyFormat", "hex")])
        );
        assert_eq!(
            hmac_of("abc", &[("key", "key"), ("output", "base64")]),
            "nBluMtwBdfhvSxy4konWYZ3mvuaZ5MN45oMJ7Zehpqs="
        );
        assert_eq!(
            hmac_of("abc", &[("key", "key"), ("uppercase", "true")]),
            "9C196E32DC0175F86F4B1CB89289D6619DE6BEE699E4C378E68309ED97A1A6AB"
        );
        let message = expect_error("hmac", "abc", &[("key", "")]);
        assert!(message.contains("密钥是空的"), "{message}");
    }

    #[test]
    fn pbkdf2_vectors() {
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[
                    ("algorithm", "sha1"),
                    ("salt", "salt"),
                    ("iterations", "1"),
                    ("keyLength", "20")
                ]
            ),
            "0c60c80f961f0e71f3a9b524af6012062fe037a6"
        );
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[
                    ("algorithm", "sha1"),
                    ("salt", "salt"),
                    ("iterations", "2"),
                    ("keyLength", "20")
                ]
            ),
            "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"
        );
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[
                    ("algorithm", "sha1"),
                    ("salt", "salt"),
                    ("iterations", "4096"),
                    ("keyLength", "20")
                ]
            ),
            "4b007901b765489abead49d926f721d065a429c1"
        );
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[("salt", "salt"), ("iterations", "1"), ("keyLength", "32")]
            ),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[("salt", "salt"), ("iterations", "2"), ("keyLength", "32")]
            ),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        assert_eq!(
            run_op_text(
                "pbkdf2",
                "password",
                &[
                    ("salt", "salt"),
                    ("iterations", "4096"),
                    ("keyLength", "32")
                ]
            ),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
        // 口令也可以来自上游步骤：把 cGFzc3dvcmQ= 先 Base64 解码成 "password" 再派生，
        // 结果必须与明文口令那条一致，否则链式用法就没法信
        let via_chain = crate::services::chef::testing::bake(
            &[
                ("base64Decode", &[]),
                (
                    "pbkdf2",
                    &[("salt", "salt"), ("iterations", "1"), ("keyLength", "32")][..],
                ),
            ],
            "cGFzc3dvcmQ=",
        )
        .output;
        assert_eq!(
            via_chain,
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        let message = expect_error(
            "pbkdf2",
            "password",
            &[("iterations", "99999999"), ("salt", "s")],
        );
        assert!(message.contains("迭代次数"), "{message}");
        let message = expect_error("pbkdf2", "password", &[("salt", "s"), ("keyLength", "0")]);
        assert!(message.contains("keyLength"), "{message}");
    }

    #[test]
    fn hkdf_rfc5869_appendix_a1() {
        let ikm = vec![0x0bu8; 22];
        let out = run_op_bytes(
            "hkdf",
            &ikm,
            &[
                ("salt", "000102030405060708090a0b0c"),
                ("saltFormat", "hex"),
                ("info", "f0f1f2f3f4f5f6f7f8f9"),
                ("infoFormat", "hex"),
                ("keyLength", "42"),
            ],
        )
        .text_lossy();
        assert_eq!(
            out,
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        // 只取前 16 字节应当是上面那串的前缀（expand 的分块语义）
        let short = run_op_bytes(
            "hkdf",
            &ikm,
            &[
                ("salt", "000102030405060708090a0b0c"),
                ("saltFormat", "hex"),
                ("info", "f0f1f2f3f4f5f6f7f8f9"),
                ("infoFormat", "hex"),
                ("keyLength", "16"),
            ],
        )
        .text_lossy();
        assert_eq!(short, "3cb25f25faacd57a90434f64d0362f2a");
    }

    #[test]
    fn bcrypt_hash_and_verify_roundtrip() {
        let hash = run_op_text("bcryptHash", "password123", &[("cost", "4")]);
        assert!(hash.starts_with("$2b$04$"), "{hash}");
        assert_eq!(hash.chars().count(), 60, "{hash}");
        assert!(
            run_op_text("bcryptVerify", "password123", &[("hash", hash.as_str())])
                .starts_with("匹配"),
            "{hash}"
        );
        assert!(
            run_op_text("bcryptVerify", "wrong-one", &[("hash", hash.as_str())])
                .starts_with("不匹配")
        );
        // 版本前缀要真的落到哈希串上（不同系统认不同前缀）
        assert!(
            run_op_text("bcryptHash", "x", &[("cost", "4"), ("version", "$2a$")])
                .starts_with("$2a$04$")
        );
    }

    #[test]
    fn bcrypt_openbsd_vector_verifies() {
        // OpenBSD 测试集里的一条固定哈希：校验路径不依赖随机盐，可以拿来做真值核对
        let hash = "$2a$06$DCq7YPn5Rq63x1Lad4cll.TV4S6ytwfsfvkgY8jIucDrjc8deX1s.";
        assert!(
            run_op_text("bcryptVerify", "", &[("hash", hash)]).starts_with("匹配"),
            "官方向量应能校验通过"
        );
        assert!(run_op_text("bcryptVerify", "secret", &[("hash", hash)]).starts_with("不匹配"));
    }

    #[test]
    fn bcrypt_rejects_oversized_password_and_bad_cost() {
        let long = "x".repeat(80);
        let message = expect_error("bcryptHash", &long, &[("cost", "4")]);
        assert!(message.contains("72"), "{message}");
        let message = expect_error("bcryptHash", "secret", &[("cost", "20")]);
        assert!(message.contains("4–15"), "{message}");
        let message = expect_error("bcryptVerify", "secret", &[("hash", "")]);
        assert!(message.contains("还差一步"), "{message}");
    }
}
