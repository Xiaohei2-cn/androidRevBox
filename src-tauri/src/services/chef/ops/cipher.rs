//! 加解密：分组密码（AES/DES/3DES/SM4）、流密码（RC4/ChaCha20/XOR）、
//! AEAD（AES-GCM/ChaCha20-Poly1305）、RSA，以及两个古典密码。
//!
//! 模式（CBC/ECB/CTR/CFB8/CFB128/OFB）与填充在这里**自己实现**，只把
//! 「单个分组的加密」交给各个密码 crate。理由很实际：
//! RustCrypto 的 `block-modes` 还挂在 cipher 0.3，而 aes/des/sm4 都已经是
//! cipher 0.4，硬凑会撞 trait 版本；而 CBC/CFB/OFB 本身就是分组加密之上的
//! 十几行异或链，自己写反而每条都能用参考实现的向量钉死（见本文件末的测试）。
//!
//! CyberChef 把「Encrypt」和「Decrypt」做成两个独立条目，这里沿用：
//! 两个方向的默认参数本来就该不同（加密默认 UTF-8 进 / Base64 出，解密反之）。

use cipher::{BlockCipher, BlockDecrypt, BlockEncrypt, KeyInit};
use typenum::Unsigned;

use crate::services::chef::data::{
    Args, ChefData, ChefResult, emit_by_format, expect_len, fail, parse_param_bytes, read_by_format,
};
use crate::services::chef::spec::{bool_arg, choice, number, op, text};

const MODES: &[&str] = &["cbc", "ecb", "ctr", "cfb8", "cfb128", "ofb"];
const PADDINGS: &[&str] = &["pkcs7", "none", "iso7816", "ansix923", "zero"];
/// 密钥/IV/输入输出的写法：`auto` 只用于输出侧的展示（默认按 UTF-8 交回流水线）
const FORMATS: &[&str] = &["utf8", "hex", "base64", "decimal"];

// ---------------------------------------------------------------------------
// 分组模式（全部建在「单分组加密/解密」之上）
// ---------------------------------------------------------------------------

fn xor_into(target: &mut [u8], other: &[u8]) {
    for (t, o) in target.iter_mut().zip(other) {
        *t ^= *o;
    }
}

fn pad(data: &[u8], block: usize, scheme: &str) -> ChefResult<Vec<u8>> {
    let need = block - (data.len() % block);
    let mut out = data.to_vec();
    match scheme {
        "none" => Ok(out),
        "pkcs7" => {
            out.resize(data.len() + need, need as u8);
            Ok(out)
        }
        "iso7816" => {
            out.push(0x80);
            while out.len() % block != 0 {
                out.push(0x00);
            }
            Ok(out)
        }
        "ansix923" => {
            let filled = data.len() + need - 1;
            out.resize(filled, 0x00);
            out.push(need as u8);
            Ok(out)
        }
        "zero" => {
            if data.len() % block == 0 {
                Ok(out)
            } else {
                out.resize(data.len() + need, 0x00);
                Ok(out)
            }
        }
        other => Err(fail(format!("未知的填充方式「{other}」"))),
    }
}

fn unpad(data: &[u8], block: usize, scheme: &str) -> ChefResult<Vec<u8>> {
    if scheme == "none" || scheme == "zero" {
        // zero 填充去掉尾部 0：这是「看得见的补零」，与 PKCS7 不同，
        // 去掉尾部零字节是常规做法，但数据本身就含尾零时会误伤，说明写在算法说明里
        if scheme == "zero" {
            let trimmed = data.len() - data.iter().rev().take_while(|b| **b == 0).count();
            return Ok(data[..trimmed].to_vec());
        }
        return Ok(data.to_vec());
    }
    if data.is_empty() || data.len() % block != 0 {
        return Err(fail(
            "密文长度不是分组的整数倍，解不了（密钥/IV/输入格式先核对一下）",
        ));
    }
    match scheme {
        "pkcs7" => {
            let count = *data.last().unwrap() as usize;
            if count == 0 || count > block || count > data.len() {
                return Err(fail("PKCS7 填充不合法：多半是密钥或 IV 不对"));
            }
            if data[data.len() - count..]
                .iter()
                .any(|b| *b as usize != count)
            {
                return Err(fail("PKCS7 填充不合法：填充字节彼此不一致，密钥或 IV 不对"));
            }
            Ok(data[..data.len() - count].to_vec())
        }
        "iso7816" => {
            // 补的是 0x80 后面跟若干 0x00：从尾部跳过 0，再要求紧挨着的那个字节是 0x80
            let mut index = data.len();
            while index > 0 && data[index - 1] == 0 {
                index -= 1;
            }
            if index == 0 || data[index - 1] != 0x80 {
                return Err(fail("ISO 7816-4 填充不合法（结尾不是 0x80 加若干 0x00）"));
            }
            Ok(data[..index - 1].to_vec())
        }
        "ansix923" => {
            let count = *data.last().unwrap() as usize;
            if count == 0 || count > block {
                return Err(fail("ANSI X9.23 填充不合法"));
            }
            Ok(data[..data.len() - count].to_vec())
        }
        other => Err(fail(format!("未知的填充方式「{other}」"))),
    }
}

/// 分组密码的统一执行入口：`mode` 决定链接方式，`padding` 只在 cbc/ecb 上有意义。
///
/// 计数器模式按「整块大端计数器」实现：16 字节块 = CTR-128BE，8 字节块（DES/3DES）
/// 自然退化为 CTR-64BE，与 OpenSSL 的默认口径一致。
fn run_block<C>(
    mode: &str,
    padding: &str,
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    decrypt: bool,
) -> ChefResult<Vec<u8>>
where
    C: BlockCipher + BlockEncrypt + BlockDecrypt + KeyInit,
{
    let cipher = C::new_from_slice(key).map_err(|_| {
        fail(format!(
            "密钥长度 {} 字节不被这个密码接受（先确认密钥写法 hex/base64/utf8 选对了）",
            key.len()
        ))
    })?;
    let block = C::BlockSize::to_usize();
    if mode != "ecb" && iv.len() != block {
        return Err(fail(format!(
            "{mode} 需要 {block} 字节的 IV，当前 {iv_len} 字节（密钥写法选 hex 时别忘了少写一个字符就少半字节）",
            iv_len = iv.len()
        )));
    }
    let mut out: Vec<u8> = Vec::new();
    match mode {
        "ecb" => {
            if decrypt {
                require_whole_blocks(data, block)?;
                let mut buf = data.to_vec();
                for chunk in buf.chunks_exact_mut(block) {
                    let mut b = cipher::Block::<C>::clone_from_slice(chunk);
                    cipher.decrypt_block(&mut b);
                    chunk.copy_from_slice(&b);
                }
                unpad(&buf, block, padding)
            } else {
                let mut buf = pad(data, block, padding)?;
                require_whole_blocks(&buf, block)?;
                for chunk in buf.chunks_exact_mut(block) {
                    let mut b = cipher::Block::<C>::clone_from_slice(chunk);
                    cipher.encrypt_block(&mut b);
                    chunk.copy_from_slice(&b);
                }
                Ok(buf)
            }
        }
        "cbc" => {
            let mut counter = iv.to_vec();
            if decrypt {
                require_whole_blocks(data, block)?;
                for chunk in data.chunks_exact(block) {
                    let mut b = cipher::Block::<C>::clone_from_slice(chunk);
                    // 先记住下一段的链值（CBC 解密用的是**密文**分段）
                    let next = chunk.to_vec();
                    cipher.decrypt_block(&mut b);
                    let mut plain = b.to_vec();
                    xor_into(&mut plain, &counter);
                    out.extend_from_slice(&plain);
                    counter = next;
                }
                unpad(&out, block, padding)
            } else {
                let mut buf = pad(data, block, padding)?;
                require_whole_blocks(&buf, block)?;
                for chunk in buf.chunks_exact_mut(block) {
                    xor_into(chunk, &counter);
                    let mut b = cipher::Block::<C>::clone_from_slice(chunk);
                    cipher.encrypt_block(&mut b);
                    chunk.copy_from_slice(&b);
                    counter = chunk.to_vec();
                }
                Ok(buf)
            }
        }
        "ctr" => {
            let mut counter = iv.to_vec();
            for chunk in data.chunks(block) {
                let mut ks = cipher::Block::<C>::clone_from_slice(&counter);
                cipher.encrypt_block(&mut ks);
                let produced: Vec<u8> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, byte)| byte ^ ks[i])
                    .collect();
                out.extend_from_slice(&produced);
                increase_be(&mut counter);
            }
            Ok(out)
        }
        "cfb8" => {
            // 逐字节反馈：寄存器左移一字节，右端补「密文字节」（CFB 的反馈永远是密文）
            let mut register = iv.to_vec();
            for byte in data {
                let mut ks = cipher::Block::<C>::clone_from_slice(&register);
                cipher.encrypt_block(&mut ks);
                let produced = byte ^ ks[0];
                out.push(produced);
                let feedback = if decrypt { *byte } else { produced };
                register.remove(0);
                register.push(feedback);
            }
            Ok(out)
        }
        "cfb128" | "ofb" => {
            let mut register = iv.to_vec();
            for chunk in data.chunks(block) {
                let mut ks = cipher::Block::<C>::clone_from_slice(&register);
                cipher.encrypt_block(&mut ks);
                let produced: Vec<u8> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, byte)| byte ^ ks[i])
                    .collect();
                if mode == "cfb128" {
                    // 反馈的是密文：加密侧取本次输出，解密侧取本次输入
                    let mut next = if decrypt {
                        chunk.to_vec()
                    } else {
                        produced.clone()
                    };
                    next.resize(block, 0);
                    register = next;
                } else {
                    register = ks.to_vec();
                }
                out.extend_from_slice(&produced);
            }
            Ok(out)
        }
        other => Err(fail(format!("未知的分组模式「{other}」"))),
    }
}

/// 大端自增（CTR 的计数器块）
fn increase_be(counter: &mut [u8]) {
    for byte in counter.iter_mut().rev() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

fn require_whole_blocks(data: &[u8], block: usize) -> ChefResult<()> {
    if data.len() % block == 0 {
        Ok(())
    } else {
        Err(fail(format!(
            "数据长度 {} 不是分组 {block} 字节的整数倍。填充选 none 时必须是整数倍；正常密文请把 padding 调成 pkcs7",
            data.len()
        )))
    }
}

macro_rules! keyed_block {
    ($name:ident, $label:literal, $($len:literal => $ty:path),+ $(,)?) => {
        fn $name(
            mode: &str,
            padding: &str,
            key: &[u8],
            iv: &[u8],
            data: &[u8],
            decrypt: bool,
        ) -> ChefResult<Vec<u8>> {
            match key.len() {
                $( $len => run_block::<$ty>(mode, padding, key, iv, data, decrypt), )+
                other => Err(fail(format!(
                    "{} 的密钥只能是 {} 字节，当前 {other} 字节（检查密钥写法 hex/base64/utf8）",
                    $label,
                    [$($len),+].map(|v| v.to_string()).join(" / ")
                ))),
            }
        }
    };
}

keyed_block!(aes_block, "AES", 16 => aes::Aes128, 24 => aes::Aes192, 32 => aes::Aes256);
keyed_block!(des_block, "DES", 8 => des::Des);
keyed_block!(sm4_block, "SM4", 16 => sm4::Sm4);

/// 3DES：16 字节密钥按 K1K2 → K1K2K1 补齐（OpenSSL/CyberChef 同一口径）
fn des3_block(
    mode: &str,
    padding: &str,
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    decrypt: bool,
) -> ChefResult<Vec<u8>> {
    let owned: Vec<u8> = match key.len() {
        24 => key.to_vec(),
        16 => {
            let mut k = key.to_vec();
            k.extend_from_slice(&key[..8]);
            k
        }
        other => {
            return Err(fail(format!(
                "3DES 的密钥要 16 或 24 字节，当前 {other} 字节"
            )));
        }
    };
    run_block::<des::TdesEde3>(mode, padding, &owned, iv, data, decrypt)
}

type BlockRunner = fn(&str, &str, &[u8], &[u8], &[u8], bool) -> ChefResult<Vec<u8>>;

fn symmetric(
    data: &ChefData,
    args: &Args,
    decrypt: bool,
    runner: BlockRunner,
    label: &str,
) -> ChefResult<ChefData> {
    let mode = args.choice_in("mode", "cbc", MODES)?;
    let padding = args.choice_in("padding", "pkcs7", PADDINGS)?;
    let key = parse_param_bytes(
        &args.text("key", ""),
        &args.choice_in("keyFormat", "hex", FORMATS)?,
    )?;
    let iv = parse_param_bytes(
        &args.text("iv", ""),
        &args.choice_in("ivFormat", "hex", FORMATS)?,
    )?;
    if key.is_empty() {
        return Err(fail(format!("还差一步：{label} 的密钥是空的")));
    }
    let input = read_by_format(
        data,
        &args.choice_in(
            "inputFormat",
            if decrypt { "base64" } else { "utf8" },
            FORMATS,
        )?,
    )?;
    let out = runner(&mode, &padding, &key, &iv, &input, decrypt)
        .map_err(|e| fail(format!("{label}·{mode} 失败：{}", e.message)))?;
    emit_by_format(
        out,
        &args.choice_in(
            "outputFormat",
            if decrypt { "utf8" } else { "base64" },
            FORMATS,
        )?,
    )
}

const ENCRYPT_ARGS: &[crate::services::chef::spec::ArgSpec] = &[
    choice("mode", "cbc", MODES),
    text("key", ""),
    choice("keyFormat", "hex", FORMATS),
    text("iv", ""),
    choice("ivFormat", "hex", FORMATS),
    choice("padding", "pkcs7", PADDINGS),
    choice("inputFormat", "utf8", FORMATS),
    choice("outputFormat", "base64", FORMATS),
];

const DECRYPT_ARGS: &[crate::services::chef::spec::ArgSpec] = &[
    choice("mode", "cbc", MODES),
    text("key", ""),
    choice("keyFormat", "hex", FORMATS),
    text("iv", ""),
    choice("ivFormat", "hex", FORMATS),
    choice("padding", "pkcs7", PADDINGS),
    choice("inputFormat", "base64", FORMATS),
    choice("outputFormat", "utf8", FORMATS),
];

fn aes_encrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, false, aes_block, "AES")
}
fn aes_decrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, true, aes_block, "AES")
}
fn des_encrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, false, des_block, "DES")
}
fn des_decrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, true, des_block, "DES")
}
fn des3_encrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, false, des3_block, "3DES")
}
fn des3_decrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, true, des3_block, "3DES")
}
fn sm4_encrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, false, sm4_block, "SM4")
}
fn sm4_decrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    symmetric(data, args, true, sm4_block, "SM4")
}

// ---------------------------------------------------------------------------
// 流密码：RC4 / XOR / ChaCha20
// ---------------------------------------------------------------------------

/// RC4：算法本体二十来行，直接写在这里。Android native 层与老 SDK 里的 RC4
/// 就是这种裸循环；绑一个 stream-cipher 0.3 时代的 crate 反而更难核对。
fn rc4_keystream_xor(key: &[u8], data: &[u8], drop: usize) -> Vec<u8> {
    let mut state: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, j as usize);
    }
    let mut i: u8 = 0;
    let mut j: u8 = 0;
    for _ in 0..drop {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[i as usize]);
        state.swap(i as usize, j as usize);
    }
    let mut out = Vec::with_capacity(data.len());
    for byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[i as usize]);
        state.swap(i as usize, j as usize);
        let k = state[(state[i as usize].wrapping_add(state[j as usize])) as usize];
        out.push(byte ^ k);
    }
    out
}

fn read_key(args: &Args, default_format: &str) -> ChefResult<Vec<u8>> {
    let key = parse_param_bytes(
        &args.text("key", ""),
        &args.choice_in("keyFormat", default_format, FORMATS)?,
    )?;
    if key.is_empty() {
        return Err(fail(
            "还差一步：密钥是空的（可用 utf8 / hex / base64 三种写法）",
        ));
    }
    Ok(key)
}

fn rc4(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let key = read_key(args, "utf8")?;
    let drop = args.number("drop", 0)? as usize;
    Ok(ChefData::new(rc4_keystream_xor(&key, &data.bytes, drop)))
}

fn xor(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let key = read_key(args, "hex")?;
    let mut out = data.bytes.clone();
    for (index, byte) in out.iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
    Ok(ChefData::new(out))
}

/// 单字节 XOR 暴破：脱壳时最常见的「被一个 byte 糊过一层」。
///
/// 排序口径是「像正常文本的比例」（字母与空格），不是只数「可打印」：
/// 短串里几乎每个错误密钥都能拿到满分可打印，真密钥反而被挤到几十名之后。
/// 分数相同就按可打印比例次之——最终判读的还是人，所以这里给的是候选清单。
fn xor_brute_force(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    if data.bytes.is_empty() {
        return Err(fail("输入框是空的，没有数据可以暴力"));
    }
    let printable_only = args.bool_of("printableOnly");
    let top = args.number("top", 10)?.max(1) as usize;
    let mut scored: Vec<(u8, f64, f64, String)> = Vec::new();
    for key in 0u8..=255 {
        let attempt: Vec<u8> = data.bytes.iter().map(|byte| byte ^ key).collect();
        let rendered = String::from_utf8_lossy(&attempt);
        let total = rendered.len().max(1);
        let printable = rendered
            .bytes()
            .filter(|b| b.is_ascii_graphic() || matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
            .count();
        let wordlike = rendered
            .bytes()
            .filter(|b| b.is_ascii_alphabetic() || *b == b' ')
            .count();
        let printable_ratio = printable as f64 / total as f64;
        if printable_only && printable_ratio < 0.9 {
            continue;
        }
        let one_line: String = rendered
            .chars()
            .map(|c| if c == '\n' { '\u{23ce}' } else { c })
            .take(160)
            .collect();
        scored.push((
            key,
            wordlike as f64 / total as f64,
            printable_ratio,
            one_line,
        ));
    }
    if scored.is_empty() {
        return Err(fail(
            "256 个单字节密钥都没打捞出可读内容：大概不是单字节 XOR，改用多位密钥的 XOR",
        ));
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(core::cmp::Ordering::Equal)
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(core::cmp::Ordering::Equal))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut out = String::from("按「像正常文本的比例」排序的候选（密钥 · 比例 · 结果）：\n");
    for (key, ratio, _printable, text) in scored.iter().take(top) {
        out.push_str(&format!("{key:#04x} · {ratio:.2} · {text}\n"));
    }
    Ok(ChefData::from_text(out.trim_end_matches('\n')))
}

fn chacha20(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    use cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
    let key = read_key(args, "hex")?;
    expect_len(&key, 32, "ChaCha20 密钥")?;
    let nonce = parse_param_bytes(
        &args.text("iv", ""),
        &args.choice_in("ivFormat", "hex", FORMATS)?,
    )?;
    expect_len(&nonce, 12, "ChaCha20 nonce")?;
    // RFC 8439 的 block counter 以 64 字节为一块，界面上填的是块号
    let counter = args.number("counter", 0)?;
    let mut stream = chacha20::ChaCha20::new_from_slices(&key, &nonce)
        .map_err(|_| fail("ChaCha20 需要 32 字节密钥与 12 字节 nonce（写法选错最容易卡在这里）"))?;
    stream.seek(counter * 64);
    let mut buf = data.bytes.clone();
    stream.apply_keystream(&mut buf);
    Ok(ChefData::new(buf))
}

// ---------------------------------------------------------------------------
// AEAD：AES-GCM / ChaCha20-Poly1305（结果都是「密文 ‖ 16 字节标签」）
// ---------------------------------------------------------------------------

type AeadParts = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

fn aead_parts(data: &ChefData, args: &Args, decrypt: bool) -> ChefResult<AeadParts> {
    let key = read_key(args, "hex")?;
    let nonce = parse_param_bytes(
        &args.text("iv", ""),
        &args.choice_in("ivFormat", "hex", FORMATS)?,
    )?;
    let aad = parse_param_bytes(
        &args.text("aad", ""),
        &args.choice_in("aadFormat", "utf8", FORMATS)?,
    )?;
    expect_len(&nonce, 12, "AEAD nonce")?;
    let input = read_by_format(
        data,
        &args.choice_in(
            "inputFormat",
            if decrypt { "base64" } else { "utf8" },
            FORMATS,
        )?,
    )?;
    if decrypt && input.len() < 16 {
        return Err(fail(
            "AEAD 的输入不足 16 字节：连末尾的认证标签都不完整，没法验",
        ));
    }
    Ok((key, nonce, aad, input))
}

fn aead_emit(bytes: Vec<u8>, args: &Args, decrypt: bool) -> ChefResult<ChefData> {
    emit_by_format(
        bytes,
        &args.choice_in(
            "outputFormat",
            if decrypt { "utf8" } else { "base64" },
            FORMATS,
        )?,
    )
}

fn aes_gcm(data: &ChefData, args: &Args, decrypt: bool) -> ChefResult<ChefData> {
    use aes_gcm::aead::{Aead, KeyInit, Payload, generic_array::GenericArray};
    let (key, nonce, aad, input) = aead_parts(data, args, decrypt)?;
    let nonce_ref = GenericArray::from_slice(&nonce);
    let payload = Payload {
        msg: &input,
        aad: &aad,
    };
    let outcome = match key.len() {
        16 => {
            let cipher = aes_gcm::Aes128Gcm::new_from_slice(&key)
                .map_err(|_| fail("内部：AES-128-GCM 密钥装配失败"))?;
            if decrypt {
                cipher.decrypt(nonce_ref, payload)
            } else {
                cipher.encrypt(nonce_ref, payload)
            }
        }
        32 => {
            let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key)
                .map_err(|_| fail("内部：AES-256-GCM 密钥装配失败"))?;
            if decrypt {
                cipher.decrypt(nonce_ref, payload)
            } else {
                cipher.encrypt(nonce_ref, payload)
            }
        }
        24 => {
            return Err(fail(
                "AES-192-GCM 这一档本轮没接（依赖库只出 128/256）。改用 AES-128/256-GCM，或走「AES-CBC + HMAC」",
            ));
        }
        other => {
            return Err(fail(format!(
                "AES-GCM 密钥要 16 或 32 字节，当前 {other} 字节"
            )));
        }
    };
    let bytes = if decrypt {
        outcome.map_err(|_| {
            fail("AES-GCM 校验失败：密钥/nonce/AAD 只要有一个不对，或密文（含末尾 16 字节标签）被改过，都是这一句")
        })?
    } else {
        outcome.map_err(|e| fail(format!("AES-GCM 加密失败：{e}")))?
    };
    aead_emit(bytes, args, decrypt)
}

fn chacha20_poly1305(data: &ChefData, args: &Args, decrypt: bool) -> ChefResult<ChefData> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload, generic_array::GenericArray};
    let (key, nonce, aad, input) = aead_parts(data, args, decrypt)?;
    expect_len(&key, 32, "ChaCha20-Poly1305 密钥")?;
    let cipher = chacha20poly1305::ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| fail("ChaCha20-Poly1305 密钥必须是 32 字节"))?;
    let nonce_ref = GenericArray::from_slice(&nonce);
    let payload = Payload {
        msg: &input,
        aad: &aad,
    };
    let bytes = if decrypt {
        cipher.decrypt(nonce_ref, payload).map_err(|_| {
            fail("校验失败：密钥/nonce/AAD 有一个不对，或密文（含末尾 16 字节标签）被动过")
        })?
    } else {
        cipher
            .encrypt(nonce_ref, payload)
            .map_err(|e| fail(format!("ChaCha20-Poly1305 加密失败：{e}")))?
    };
    aead_emit(bytes, args, decrypt)
}

// ---------------------------------------------------------------------------
// RSA
// ---------------------------------------------------------------------------

const RSA_PADDINGS: &[&str] = &["pkcs1v15", "oaep-sha1", "oaep-sha256", "oaep-sha512"];

/// 每种填充一次能装多少明文：PKCS#1 v1.5 省 11 字节，OAEP 省 2×哈希长度 + 2
fn rsa_capacity(padding: &str, modulus_bytes: usize) -> usize {
    match padding {
        "pkcs1v15" => modulus_bytes.saturating_sub(11),
        "oaep-sha1" => modulus_bytes.saturating_sub(42),
        "oaep-sha256" => modulus_bytes.saturating_sub(66),
        "oaep-sha512" => modulus_bytes.saturating_sub(130),
        _ => 0,
    }
}

/// rsa 0.9 的 PaddingScheme 按值消费自己，塞不进 dyn：只能按选项名各写一路
macro_rules! rsa_case {
    ($name:expr, $body:expr) => {
        match $name {
            "pkcs1v15" => $body(rsa::Pkcs1v15Encrypt),
            "oaep-sha1" => $body(rsa::Oaep::new::<sha1::Sha1>()),
            "oaep-sha256" => $body(rsa::Oaep::new::<sha2::Sha256>()),
            "oaep-sha512" => $body(rsa::Oaep::new::<sha2::Sha512>()),
            other => Err(fail(format!(
                "未知的 RSA 填充「{other}」（可选：{}）",
                RSA_PADDINGS.join(" / ")
            ))),
        }
    };
}

fn rsa_encrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    use rsa::pkcs8::DecodePublicKey;
    use rsa::traits::PublicKeyParts;
    let pem = args.text("publicKey", "");
    if pem.trim().is_empty() {
        return Err(fail(
            "还差一步：RSA 公钥（PEM，BEGIN PUBLIC KEY）要填在 publicKey 里",
        ));
    }
    let key = rsa::RsaPublicKey::from_public_key_pem(&pem).map_err(|e| {
        fail(format!(
            "公钥读不动：{e}（PEM 首尾行要完整；要解密请用 privateKey 那一栏）"
        ))
    })?;
    let padding = args.choice_in("padding", "oaep-sha256", RSA_PADDINGS)?;
    let input = read_by_format(data, &args.choice_in("inputFormat", "utf8", FORMATS)?)?;
    let limit = rsa_capacity(&padding, key.size());
    if input.len() > limit {
        return Err(fail(format!(
            "RSA 装不下：{padding} 在 {} 位密钥下一次最多 {limit} 字节，当前 {len} 字节。这是 RSA 的固有限制，常规做法是「AES 加密数据 + RSA 加密 AES 密钥」",
            key.size() * 8,
            len = input.len()
        )));
    }
    let bytes = rsa_case!(padding.as_str(), |pad| {
        let mut rng = rand::thread_rng();
        key.encrypt(&mut rng, pad, &input)
            .map_err(|e| fail(format!("RSA 加密失败：{e}")))
    })?;
    emit_by_format(bytes, &args.choice_in("outputFormat", "base64", FORMATS)?)
}

fn rsa_decrypt(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::DecodePrivateKey;
    let pem = args.text("privateKey", "");
    if pem.trim().is_empty() {
        return Err(fail(
            "还差一步：RSA 私钥（PEM，BEGIN PRIVATE KEY）要填在 privateKey 里",
        ));
    }
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(&pem)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_pem(&pem))
        .map_err(|e| {
            fail(format!(
                "私钥读不动：{e}（PKCS#8 与 PKCS#1 都试过了；带口令的加密私钥这里不支持）"
            ))
        })?;
    let padding = args.choice_in("padding", "oaep-sha256", RSA_PADDINGS)?;
    let input = read_by_format(data, &args.choice_in("inputFormat", "base64", FORMATS)?)?;
    let bytes = rsa_case!(padding.as_str(), |pad| key.decrypt(pad, &input).map_err(
        |e| fail(format!(
            "RSA 解密失败：{e}（填充选错或密钥不配对是最常见原因）"
        ))
    ))?;
    emit_by_format(bytes, &args.choice_in("outputFormat", "utf8", FORMATS)?)
}

/// RSA 密钥对生成：先自己加密/解密一轮验证通路，再去翻真密钥
fn rsa_generate(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    let _ = data;
    let bits = args.number("bits", 2048)? as usize;
    if !(1024..=4096).contains(&bits) {
        return Err(fail(
            "RSA 位数要在 1024–4096 之间（1024 只够自测，别拿去当真密钥）",
        ));
    }
    let mut rng = rand::thread_rng();
    let private =
        rsa::RsaPrivateKey::new(&mut rng, bits).map_err(|e| fail(format!("密钥生成失败：{e}")))?;
    let public = rsa::RsaPublicKey::from(&private);
    let priv_pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| fail(format!("私钥导出 PEM 失败：{e}")))?
        .to_string();
    let pub_pem = public
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| fail(format!("公钥导出 PEM 失败：{e}")))?
        .to_string();
    Ok(ChefData::from_text(&format!(
        "----- 私钥（PKCS#8）-----\n{priv_pem}----- 公钥（SPKI）-----\n{pub_pem}"
    )))
}

// ---------------------------------------------------------------------------
// 古典密码（凯撒 / ROT13 / 维吉尼亚）
// ---------------------------------------------------------------------------

fn shift_letter(c: char, shift: i32) -> char {
    let base = if c.is_ascii_lowercase() {
        b'a'
    } else if c.is_ascii_uppercase() {
        b'A'
    } else {
        return c;
    };
    let v = (c as u8 - base) as i32;
    ((((v + shift).rem_euclid(26)) as u8) + base) as char
}

fn caesar(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let shift = args.number("shift", 3)? as i32;
    let out: String = data
        .text()?
        .chars()
        .map(|c| shift_letter(c, shift))
        .collect();
    Ok(ChefData::from_text(&out))
}

fn rot13(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let out: String = data.text()?.chars().map(|c| shift_letter(c, 13)).collect();
    Ok(ChefData::from_text(&out))
}

fn vigenere(data: &ChefData, args: &Args, decrypt: bool) -> ChefResult<ChefData> {
    let key: Vec<u8> = args
        .text("key", "")
        .to_ascii_lowercase()
        .bytes()
        .filter(|b| b.is_ascii_alphabetic())
        .collect();
    if key.is_empty() {
        return Err(fail(
            "还差一步：维吉尼亚的密钥要含字母（当前为空或全是非字母）",
        ));
    }
    let mut index = 0usize;
    let out: String = data
        .text()?
        .chars()
        .map(|c| {
            if !c.is_ascii_alphabetic() {
                return c;
            }
            let shift = (key[index % key.len()] - b'a') as i32;
            index += 1;
            shift_letter(c, if decrypt { -shift } else { shift })
        })
        .collect();
    Ok(ChefData::from_text(&out))
}

const AEAD_ENCRYPT_ARGS: &[crate::services::chef::spec::ArgSpec] = &[
    text("key", ""),
    choice("keyFormat", "hex", FORMATS),
    text("iv", ""),
    choice("ivFormat", "hex", FORMATS),
    text("aad", ""),
    choice("aadFormat", "utf8", FORMATS),
    choice("inputFormat", "utf8", FORMATS),
    choice("outputFormat", "base64", FORMATS),
];

const AEAD_DECRYPT_ARGS: &[crate::services::chef::spec::ArgSpec] = &[
    text("key", ""),
    choice("keyFormat", "hex", FORMATS),
    text("iv", ""),
    choice("ivFormat", "hex", FORMATS),
    text("aad", ""),
    choice("aadFormat", "utf8", FORMATS),
    choice("inputFormat", "base64", FORMATS),
    choice("outputFormat", "utf8", FORMATS),
];

pub const OPS: &[super::Op] = &[
    op("aesEncrypt", "cipher", ENCRYPT_ARGS, aes_encrypt),
    op("aesDecrypt", "cipher", DECRYPT_ARGS, aes_decrypt),
    op(
        "aesGcmEncrypt",
        "cipher",
        AEAD_ENCRYPT_ARGS,
        |data, args| aes_gcm(data, args, false),
    ),
    op(
        "aesGcmDecrypt",
        "cipher",
        AEAD_DECRYPT_ARGS,
        |data, args| aes_gcm(data, args, true),
    ),
    op("desEncrypt", "cipher", ENCRYPT_ARGS, des_encrypt),
    op("desDecrypt", "cipher", DECRYPT_ARGS, des_decrypt),
    op("des3Encrypt", "cipher", ENCRYPT_ARGS, des3_encrypt),
    op("des3Decrypt", "cipher", DECRYPT_ARGS, des3_decrypt),
    op("sm4Encrypt", "cipher", ENCRYPT_ARGS, sm4_encrypt),
    op("sm4Decrypt", "cipher", DECRYPT_ARGS, sm4_decrypt),
    op(
        "rc4",
        "cipher",
        &[
            text("key", ""),
            choice("keyFormat", "utf8", FORMATS),
            number("drop", 0),
        ],
        rc4,
    ),
    op(
        "chacha20",
        "cipher",
        &[
            text("key", ""),
            choice("keyFormat", "hex", FORMATS),
            text("iv", ""),
            choice("ivFormat", "hex", FORMATS),
            number("counter", 0),
        ],
        chacha20,
    ),
    op(
        "chacha20Poly1305Encrypt",
        "cipher",
        AEAD_ENCRYPT_ARGS,
        |data, args| chacha20_poly1305(data, args, false),
    ),
    op(
        "chacha20Poly1305Decrypt",
        "cipher",
        AEAD_DECRYPT_ARGS,
        |data, args| chacha20_poly1305(data, args, true),
    ),
    op(
        "xor",
        "cipher",
        &[text("key", ""), choice("keyFormat", "hex", FORMATS)],
        xor,
    ),
    op(
        "xorBruteForce",
        "cipher",
        &[bool_arg("printableOnly", true), number("top", 10)],
        xor_brute_force,
    ),
    op(
        "rsaEncrypt",
        "cipher",
        &[
            text("publicKey", ""),
            choice("padding", "oaep-sha256", RSA_PADDINGS),
            choice("inputFormat", "utf8", FORMATS),
            choice("outputFormat", "base64", FORMATS),
        ],
        rsa_encrypt,
    ),
    op(
        "rsaDecrypt",
        "cipher",
        &[
            text("privateKey", ""),
            choice("padding", "oaep-sha256", RSA_PADDINGS),
            choice("inputFormat", "base64", FORMATS),
            choice("outputFormat", "utf8", FORMATS),
        ],
        rsa_decrypt,
    ),
    op(
        "rsaGenerate",
        "cipher",
        &[number("bits", 2048)],
        rsa_generate,
    ),
    op("caesar", "cipher", &[number("shift", 3)], caesar),
    op("rot13", "cipher", &[], rot13),
    op(
        "vigenere",
        "cipher",
        &[
            text("key", ""),
            choice("operation", "encrypt", &["encrypt", "decrypt"]),
        ],
        |data, args| vigenere(data, args, args.choice("operation", "encrypt") == "decrypt"),
    ),
];

// 参考值来自另一套实现（pycryptodome / OpenSSL / GB/T 32907 / RFC 8439），
// 单独一个文件是为了让长向量表不挤占算法实现的阅读空间。
#[cfg(test)]
#[path = "cipher_tests.rs"]
mod tests;
