//! 加解密算法的参考向量测试。
//!
//! 期望值一律来自**另一套实现**（pycryptodome / OpenSSL / GB/T 32907 / RFC 8439），
//! 不是拿本文件的代码再算一遍——自己算自己只能证明稳定，证明不了正确。

use crate::services::chef::testing::{expect_error, run_op_bytes, run_op_text};

const K16: &str = "2b7e151628aed2a6abf7158809cf4f3c";
const K32: &str = "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4";
const IV: &str = "000102030405060708090a0b0c0d0e0f";
/// 21 字节：故意跨两个分组，尾组的填充与反馈路径才会被走到
const MSG: &str = "Hello, 算法中心!";
const MSG_HEX: &str = "48656c6c6f2c20e7ae97e6b395e4b8ade5bf8321";
const BLOCK: &str = "6bc1bee2243bf96b71577bbf6f8c2594";
const CC_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const SM4_KEY: &str = "0123456789abcdeffedcba9876543210";
const DES3_24: &str = "0123456789abcdeffedcba98765432103322110044556677";

/// 期望的二进制结果（流密码与 XOR 的输出就是字节，测试按字节比）
fn bytes_of(hex_text: &str) -> Vec<u8> {
    hex::decode(hex_text).expect("测试里的 hex 必须合法")
}

#[test]
fn aes_cbc_and_ecb_match_reference_vectors() {
    assert_eq!(
        run_op_text(
            "aesEncrypt",
            MSG,
            &[
                ("key", K16),
                ("iv", IV),
                ("mode", "cbc"),
                ("padding", "pkcs7")
            ]
        ),
        "C+gB+FVNyD+ffozqQnibwaRn4xn/MYNayl8V9rxTk64="
    );
    for (key, expected) in [
        (K16, "f7d73cbe6568a1b32b572b7324dbf21e"),
        (K32, "b90cbf2bec674e9924f2e4cf93023e19"),
    ] {
        assert_eq!(
            run_op_text(
                "aesEncrypt",
                BLOCK,
                &[
                    ("key", key),
                    ("mode", "ecb"),
                    ("padding", "none"),
                    ("inputFormat", "hex"),
                    ("outputFormat", "hex"),
                ]
            ),
            expected,
            "密钥 {key}"
        );
    }
}

#[test]
fn aes_stream_modes_match_reference_vectors() {
    for (mode, expected) in [
        ("ctr", "189b0ba0f6411251749ed15a0e4b54cd4a9e0d6f"),
        ("ofb", "189b0ba0f6411251749ed15a0e4b54cd3c1b59fb"),
        ("cfb128", "189b0ba0f6411251749ed15a0e4b54cdd6ad1a18"),
        ("cfb8", "18bd6825311d8b837b53bf45be87d71b1c58f9bc"),
    ] {
        assert_eq!(
            run_op_text(
                "aesEncrypt",
                MSG_HEX,
                &[
                    ("key", K16),
                    ("iv", IV),
                    ("mode", mode),
                    ("inputFormat", "hex"),
                    ("outputFormat", "hex"),
                ]
            ),
            expected,
            "模式 {mode}"
        );
    }
}

#[test]
fn every_mode_and_padding_round_trips() {
    // 加密→解密逐组合过一遍。模式或填充写反（比如 CFB 把明文当反馈）在这种遍历里
    // 一定暴露，而单条向量不一定碰得到那一路。
    for mode in ["cbc", "ecb", "ctr", "cfb8", "cfb128", "ofb"] {
        for padding in ["pkcs7", "none", "iso7816", "ansix923", "zero"] {
            // 不填充时输入必须整块，其余填充不限长度
            let input = if padding == "none" { BLOCK } else { MSG_HEX };
            let pairs = [
                ("key", K16),
                ("iv", IV),
                ("mode", mode),
                ("padding", padding),
                ("inputFormat", "hex"),
                ("outputFormat", "hex"),
            ];
            let ct = run_op_text("aesEncrypt", input, &pairs);
            let back = run_op_text("aesDecrypt", &ct, &pairs);
            assert_eq!(back, input, "模式 {mode} 填充 {padding}");
        }
    }
}

#[test]
fn des_3des_and_sm4_vectors() {
    assert_eq!(
        run_op_text(
            "desEncrypt",
            "abcdefgh",
            &[
                ("key", "0e0a0b0e0a0b0e0a"),
                ("mode", "ecb"),
                ("padding", "none"),
                ("outputFormat", "hex"),
            ]
        ),
        "e47f514d49380de3"
    );
    assert_eq!(
        run_op_text(
            "des3Encrypt",
            "abcdefgh",
            &[
                ("key", DES3_24),
                ("iv", "0000000000000000"),
                ("padding", "none"),
                ("outputFormat", "hex"),
            ]
        ),
        "f391e11cf6eb68c5"
    );
    // 16 字节密钥按 K1K2K1 补齐（OpenSSL 口径）：与另一套实现算出的同一串一致
    assert_eq!(
        run_op_text(
            "des3Encrypt",
            "abcdefgh",
            &[
                ("key", SM4_KEY),
                ("iv", "0000000000000000"),
                ("padding", "none"),
                ("outputFormat", "hex"),
            ]
        ),
        "43eb1bd9b6c1c9e6"
    );
    // GB/T 32907-2016 的官方单块向量（ECB）+ 一条 CBC
    assert_eq!(
        run_op_text(
            "sm4Encrypt",
            SM4_KEY,
            &[
                ("key", SM4_KEY),
                ("mode", "ecb"),
                ("padding", "none"),
                ("inputFormat", "hex"),
                ("outputFormat", "hex"),
            ]
        ),
        "681edf34d206965e86b3e94f536e4246"
    );
    let sm4_cbc = run_op_text(
        "sm4Encrypt",
        SM4_KEY,
        &[
            ("key", SM4_KEY),
            ("iv", IV),
            ("padding", "none"),
            ("inputFormat", "hex"),
            ("outputFormat", "hex"),
        ],
    );
    assert_eq!(sm4_cbc, "a9a268883a336315bac0c9c9ff350ab1");
    assert_eq!(
        run_op_text(
            "sm4Decrypt",
            &sm4_cbc,
            &[
                ("key", SM4_KEY),
                ("iv", IV),
                ("padding", "none"),
                ("inputFormat", "hex"),
                ("outputFormat", "hex"),
            ]
        ),
        SM4_KEY
    );
}

#[test]
fn aead_vectors_and_tamper_detection() {
    let gcm = run_op_text(
        "aesGcmEncrypt",
        MSG,
        &[
            ("key", K16),
            ("iv", "cafebabefacedbaddecaf888"),
            ("aad", "aad"),
        ],
    );
    assert_eq!(gcm, "SWMLeTtwNTLo8X6V3ejK5co/6493w4PgDMPWOWmm4CuOI2sg");
    let ccpoly = run_op_text(
        "chacha20Poly1305Encrypt",
        MSG,
        &[
            ("key", CC_KEY),
            ("iv", "000000000000004a00000000"),
            ("aad", "more light"),
        ],
    );
    assert_eq!(ccpoly, "aio9ny83+QaBScHcLYelQGmsnKMl8fbWg0FD7FrPP4vzL5h2");
    // 同一份参数往回解，才算真的接上
    assert_eq!(
        run_op_text(
            "aesGcmDecrypt",
            &gcm,
            &[
                ("key", K16),
                ("iv", "cafebabefacedbaddecaf888"),
                ("aad", "aad")
            ]
        ),
        MSG
    );
    assert_eq!(
        run_op_text(
            "chacha20Poly1305Decrypt",
            &ccpoly,
            &[
                ("key", CC_KEY),
                ("iv", "000000000000004a00000000"),
                ("aad", "more light")
            ]
        ),
        MSG
    );
    // AAD 改一个字节就必须验不过：AEAD 存在的意义就在这
    let message = expect_error(
        "aesGcmDecrypt",
        &gcm,
        &[
            ("key", K16),
            ("iv", "cafebabefacedbaddecaf888"),
            ("aad", "aad!"),
        ],
    );
    assert!(message.contains("校验失败"), "{message}");
    let message = expect_error(
        "chacha20Poly1305Decrypt",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        &[("key", CC_KEY), ("iv", "000000000000004a00000000")],
    );
    assert!(message.contains("校验失败"), "{message}");
}

#[test]
fn chacha20_matches_rfc8439_vector() {
    // RFC 8439 §2.4.2：key=00..1f、nonce=000000000000004a00000000、块计数=1
    let msg = "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let pairs = [
        ("key", CC_KEY),
        ("iv", "000000000000004a00000000"),
        ("counter", "1"),
    ];
    let out = run_op_bytes("chacha20", msg.as_bytes(), &pairs).bytes;
    assert_eq!(hex::encode(&out[..16]), "6e2e359a2568f98041ba0728dd0d6981");
    // 流密码自反：同一份参数再跑一次应当拿回原文
    assert_eq!(run_op_bytes("chacha20", &out, &pairs).bytes, msg.as_bytes());
    let message = expect_error(
        "chacha20",
        "abc",
        &[("key", "11"), ("iv", "000000000000004a00000000")],
    );
    assert!(message.contains("32"), "{message}");
}

#[test]
fn rc4_and_xor_work_on_bytes() {
    // 公开发表的 RC4 测试串
    assert_eq!(
        run_op_bytes("rc4", b"pedia", &[("key", "Wiki")]).bytes,
        bytes_of("1021bf0420")
    );
    assert_eq!(
        run_op_bytes("rc4", b"Attack at dawn", &[("key", "Secret")]).bytes,
        bytes_of("45a01f645fc35b383552544b9bf5")
    );
    // drop=3 就是 RC4Drop：丢掉 keystream 前 3 字节
    assert_ne!(
        run_op_bytes("rc4", b"pedia", &[("key", "Wiki"), ("drop", "3")]).bytes,
        bytes_of("1021bf0420")
    );
    // 单字节 XOR 暴破要把人话捞出来，并说清用的是哪个密钥
    let cipher = run_op_bytes("xor", b"attack at dawn", &[("key", "42")]).bytes;
    let found = run_op_bytes("xorBruteForce", &cipher, &[("top", "5")]).text_lossy();
    // 排在最前面的就必须是 0x42：只看「可打印」的话，随便一个错误密钥也是满分，
    // 真密钥会被挤到几十名之后（这条断言钉住排序口径）
    let first = found.lines().nth(1).unwrap_or_default();
    assert!(first.starts_with("0x42"), "{found}");
    assert!(found.contains("attack at dawn"), "{found}");
    // 多位密钥的 XOR 是自反的
    let data = b"reverse-me";
    let enc = run_op_bytes("xor", data, &[("key", "0deadbeef")]).bytes;
    assert_ne!(enc.to_vec(), data.to_vec());
    assert_eq!(
        run_op_bytes("xor", &enc, &[("key", "0deadbeef")]).bytes,
        data.to_vec()
    );
    assert!(expect_error("xor", "abc", &[("key", "")]).contains("密钥是空的"));
}

#[test]
fn classical_ciphers() {
    assert_eq!(run_op_text("caesar", "abcXYZ", &[("shift", "3")]), "defABC");
    assert_eq!(run_op_text("rot13", "Hello, World!", &[]), "Uryyb, Jbeyq!");
    assert_eq!(run_op_text("rot13", "Uryyb, Jbeyq!", &[]), "Hello, World!");
    assert_eq!(
        run_op_text("vigenere", "attackatdawn", &[("key", "lemon")]),
        "lxfopvefrnhr"
    );
    assert_eq!(
        run_op_text(
            "vigenere",
            "lxfopvefrnhr",
            &[("key", "lemon"), ("operation", "decrypt")]
        ),
        "attackatdawn"
    );
    // 非字母原样保留，密钥只在字母上推进（维吉尼亚的标准定义）
    assert_eq!(run_op_text("vigenere", "ab cd", &[("key", "ab")]), "ac ce");
    assert!(expect_error("vigenere", "abc", &[("key", "123")]).contains("字母"));
}

#[test]
fn key_and_iv_mistakes_are_named_not_generic() {
    let message = expect_error("aesEncrypt", "abc", &[("key", ""), ("iv", IV)]);
    assert!(message.contains("密钥是空的"), "{message}");
    let message = expect_error("aesEncrypt", "abc", &[("key", "11"), ("iv", IV)]);
    assert!(message.contains("16 / 24 / 32"), "{message}");
    let message = expect_error(
        "aesEncrypt",
        "abc",
        &[("key", K16), ("iv", ""), ("mode", "cbc")],
    );
    assert!(message.contains("IV"), "{message}");
    // 不填充时非整块输入必须被拦住
    let message = expect_error(
        "aesEncrypt",
        MSG,
        &[("key", K16), ("iv", IV), ("padding", "none")],
    );
    assert!(message.contains("整数倍"), "{message}");
    // 密钥不对时报的是「解不开」，绝不能返回一段看着正常的乱码
    let ct = run_op_text("aesEncrypt", MSG, &[("key", K16), ("iv", IV)]);
    let message = expect_error("aesDecrypt", &ct, &[("key", K32), ("iv", IV)]);
    assert!(
        message.contains("填充") || message.contains("整数倍"),
        "{message}"
    );
}

#[test]
fn rsa_generates_parses_and_round_trips() {
    let generated = run_op_text("rsaGenerate", "", &[("bits", "2048")]);
    let (priv_part, public) = generated
        .split_once("----- 公钥（SPKI）-----\n")
        .expect("生成的文本里有公钥段");
    let private = priv_part
        .strip_prefix("----- 私钥（PKCS#8）-----\n")
        .expect("生成的文本里有私钥段");
    assert!(public.starts_with("-----BEGIN PUBLIC KEY-----"), "{public}");
    assert!(private.starts_with("-----BEGIN PRIVATE KEY-----"));
    for padding in ["pkcs1v15", "oaep-sha1", "oaep-sha256", "oaep-sha512"] {
        let ct = run_op_text(
            "rsaEncrypt",
            "rsa-hello",
            &[("publicKey", public), ("padding", padding)],
        );
        let back = run_op_text(
            "rsaDecrypt",
            &ct,
            &[("privateKey", private), ("padding", padding)],
        );
        assert_eq!(back, "rsa-hello", "填充 {padding}");
    }
    // 超出容量必须说「装不下」并给出常规做法，而不是笼统报失败
    let long = "x".repeat(300);
    let message = expect_error("rsaEncrypt", &long, &[("publicKey", public)]);
    assert!(message.contains("装不下"), "{message}");
    // 填充选错不能返回一段假明文
    let ct = run_op_text(
        "rsaEncrypt",
        "rsa-hello",
        &[("publicKey", public), ("padding", "pkcs1v15")],
    );
    let message = expect_error(
        "rsaDecrypt",
        &ct,
        &[("privateKey", private), ("padding", "oaep-sha256")],
    );
    assert!(message.contains("解密失败"), "{message}");
    assert!(expect_error("rsaGenerate", "", &[("bits", "512")]).contains("1024"));
    assert!(expect_error("rsaEncrypt", "abc", &[("publicKey", "")]).contains("publicKey"));
    assert!(expect_error("rsaDecrypt", "abc", &[("privateKey", "not a pem")]).contains("私钥"));
}
