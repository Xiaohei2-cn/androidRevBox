//! 压缩与解压：gzip / zlib / raw deflate / bzip2 / brotli。
//!
//! 全部按**字节**进出：压缩产物几乎都是二进制，界面右侧把它按 hex/Base64 展示即可，
//! 不需要在算法里偷偷转成文本（那正是「看起来成功了但结果不对」的常见来源）。
//!
//! 本轮只做 bzip2 的解压：纯 Rust 的 `bzip2-rs` 只给了 decoder，
//! 要压缩就得挂 C 版 libbzip2。APK/固件里遇到 bzip2 基本都是要解开的，
//! 先把这一半做扎实；需要压回去时清单里有 gzip/zlib/brotli 可选。

use std::io::{Read, Write};

use crate::services::chef::data::{Args, ChefData, ChefResult, fail};
use crate::services::chef::spec::{number, op};

/// flate2 的压缩级别：1–9，默认 6（与 gzip 命令行一致）
fn level(args: &Args) -> flate2::Compression {
    let value = args.number("level", 6).unwrap_or(6);
    flate2::Compression::new(value.clamp(0, 9) as u32)
}

/// 压缩侧统一出口：任何 IO 错误都换成人话（压缩极少失败，失败基本都是级别参数）
fn packed(result: std::io::Result<Vec<u8>>) -> ChefResult<Vec<u8>> {
    result.map_err(|e| fail(format!("压缩失败：{e}")))
}

/// 解压侧统一出口：把「这段数据可能压根没被压过」说在前面，
/// 因为库给出的错误（invalid block type / corrupt input）对使用者毫无指导性
fn unpacked(mut reader: impl Read, what: &str) -> ChefResult<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out).map_err(|e| {
        fail(format!(
            "{what} 解压失败：{e}。常见原因是这段数据本来就没被压过，或输入格式（hex / Base64）没选对"
        ))
    })?;
    Ok(out)
}

fn gzip(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), level(args));
    enc.write_all(&data.bytes)
        .map_err(|e| fail(format!("gzip 压缩失败：{e}")))?;
    Ok(ChefData::new(packed(enc.finish())?))
}

fn gunzip(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::new(unpacked(
        flate2::read::GzDecoder::new(&data.bytes[..]),
        "gzip",
    )?))
}

/// 多段拼接的 .gz（抓包与日志目录里常见的分片写法）
fn multi_gunzip(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::new(unpacked(
        flate2::read::MultiGzDecoder::new(&data.bytes[..]),
        "gzip（多段）",
    )?))
}

fn zlib_compress(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), level(args));
    enc.write_all(&data.bytes)
        .map_err(|e| fail(format!("zlib 压缩失败：{e}")))?;
    Ok(ChefData::new(packed(enc.finish())?))
}

fn zlib_decompress(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::new(unpacked(
        flate2::read::ZlibDecoder::new(&data.bytes[..]),
        "zlib",
    )?))
}

fn deflate(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), level(args));
    enc.write_all(&data.bytes)
        .map_err(|e| fail(format!("deflate 压缩失败：{e}")))?;
    Ok(ChefData::new(packed(enc.finish())?))
}

fn inflate(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    Ok(ChefData::new(unpacked(
        flate2::read::DeflateDecoder::new(&data.bytes[..]),
        "deflate",
    )?))
}

fn brotli_compress(data: &ChefData, args: &Args) -> ChefResult<ChefData> {
    // brotli 的 quality 0–11，默认 11（H5 包体常见档位）；lgwin 24 是上限窗口
    let quality = args.number("level", 11)?.clamp(0, 11) as u32;
    let mut buf = Vec::new();
    {
        let mut enc = brotli::CompressorWriter::new(&mut buf, 4096, quality, 24);
        enc.write_all(&data.bytes)
            .map_err(|e| fail(format!("brotli 压缩失败：{e}")))?;
        enc.flush()
            .map_err(|e| fail(format!("brotli 压缩收尾失败：{e}")))?;
    }
    Ok(ChefData::new(buf))
}

fn brotli_decompress(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let mut out = Vec::new();
    let mut dec = brotli::Decompressor::new(&data.bytes[..], 4096);
    dec.read_to_end(&mut out).map_err(|e| {
        fail(format!(
            "brotli 解压失败：{e}。H5 资源要带 Content-Encoding: br 才是 brotli，先看一眼响应头"
        ))
    })?;
    Ok(ChefData::new(out))
}

fn bzip2_decompress(data: &ChefData, _args: &Args) -> ChefResult<ChefData> {
    let mut out = Vec::new();
    bzip2_rs::DecoderReader::new(&data.bytes[..])
        .read_to_end(&mut out)
        .map_err(|e| {
            fail(format!(
                "bzip2 解压失败：{e}（本轮只支持解压，需要压缩请改用 gzip/zlib/brotli）"
            ))
        })?;
    Ok(ChefData::new(out))
}

const LEVEL_ARG: &[crate::services::chef::spec::ArgSpec] = &[number("level", 6)];

pub const OPS: &[super::Op] = &[
    op("gzip", "compression", LEVEL_ARG, gzip),
    op("gunzip", "compression", &[], gunzip),
    op("multiGunzip", "compression", &[], multi_gunzip),
    op("zlibCompress", "compression", LEVEL_ARG, zlib_compress),
    op("zlibDecompress", "compression", &[], zlib_decompress),
    op("deflate", "compression", LEVEL_ARG, deflate),
    op("inflate", "compression", &[], inflate),
    op(
        "brotliCompress",
        "compression",
        &[number("level", 11)],
        brotli_compress,
    ),
    op("brotliDecompress", "compression", &[], brotli_decompress),
    op("bzip2Decompress", "compression", &[], bzip2_decompress),
];

#[cfg(test)]
mod tests {
    use crate::services::chef::testing::{expect_error, run_op_bytes, run_op_text};

    const TEXT: &str = "algorithm center 算法中心 · algorithm center 算法中心";
    /// 同一句话的 bzip2 结果（命令行 bzip2 -9 产出，Base64 存放）：
    /// 本模块不实现 bzip2 压缩，所以解压路径只能拿外部参考实现来钉
    const BZIP2_B64: &str = "QlpoOTFBWSZTWW4iL6wAAAMR/UAAKueUAAgAAoAAAwjAkAAHgCAAVDVNMAAAGglU9TRoNH6kMTTJLYXI2WDxHAxmakDcgMIQihMmULlz6wQxwECBx4XckU4UJBuIi+sA";

    #[test]
    fn each_format_round_trips() {
        for (compress, decompress) in [
            ("gzip", "gunzip"),
            ("zlibCompress", "zlibDecompress"),
            ("deflate", "inflate"),
            ("brotliCompress", "brotliDecompress"),
        ] {
            // 容器本身有几十字节开销，短文本压完反而变大是正常结果：
            // 这里量的是「同一份数据压完能原样回来」，另有一测试验长文本确实变小
            let packed = run_op_bytes(compress, TEXT.as_bytes(), &[]).bytes;
            assert!(!packed.is_empty(), "{compress} 给了空结果");
            assert_eq!(
                run_op_bytes(decompress, &packed, &[]).bytes,
                TEXT.as_bytes(),
                "{compress} → {decompress}"
            );
        }
    }

    #[test]
    fn gzip_container_is_real_gzip_and_multi_variant_chains() {
        let packed = run_op_bytes("gzip", TEXT.as_bytes(), &[]).bytes;
        assert_eq!(&packed[..2], &[0x1f, 0x8b], "gzip 魔数");
        assert_eq!(
            run_op_bytes("multiGunzip", &packed, &[]).bytes,
            TEXT.as_bytes()
        );
        // 两段拼在一起：只有 multi 那一档能全解出来
        let mut both = packed.clone();
        both.extend_from_slice(&packed);
        assert_eq!(
            run_op_bytes("multiGunzip", &both, &[]).bytes.len(),
            TEXT.len() * 2
        );
    }

    #[test]
    fn bzip2_decodes_external_stream() {
        // 解压链：Base64 解码 → bzip2 解压，全用本项目的算法接起来
        let packed = run_op_bytes("base64Decode", BZIP2_B64.as_bytes(), &[]).bytes;
        assert_eq!(&packed[..3], b"BZh", "bzip2 魔数");
        assert_eq!(
            run_op_bytes("bzip2Decompress", &packed, &[]).text_lossy(),
            TEXT
        );
    }

    #[test]
    fn long_repetitive_text_actually_shrinks() {
        // 压缩到底有没有在工作，用可重复的长文本量：这一条挂了就是「假装压缩」
        let big = (TEXT.to_string() + " ").repeat(200);
        for compress in ["gzip", "zlibCompress", "deflate", "brotliCompress"] {
            let packed = run_op_bytes(compress, big.as_bytes(), &[]).bytes;
            assert!(
                packed.len() * 8 < big.len(),
                "{compress} 压缩比异常：{} → {}",
                big.len(),
                packed.len()
            );
        }
    }

    #[test]
    fn levels_and_bad_input() {
        let low = run_op_bytes("gzip", TEXT.as_bytes(), &[("level", "1")]).bytes;
        let high = run_op_bytes("gzip", TEXT.as_bytes(), &[("level", "9")]).bytes;
        assert!(!low.is_empty() && !high.is_empty());
        assert!(run_op_text("zlibCompress", TEXT, &[("level", "0")]).len() > 8);
        let message = expect_error("gunzip", "这不是 gzip", &[]);
        assert!(message.contains("解压失败"), "{message}");
        let message = expect_error("brotliDecompress", "nope", &[]);
        assert!(message.contains("brotli"), "{message}");
        let message = expect_error("bzip2Decompress", "nope", &[]);
        assert!(message.contains("bzip2"), "{message}");
    }
}
