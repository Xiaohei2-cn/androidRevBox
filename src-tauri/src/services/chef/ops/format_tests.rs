use crate::services::chef::testing::{expect_error, run_op_bytes, run_op_text};

fn from_hex(text: &str) -> Vec<u8> {
    hex::decode(text).expect("测试里的 hex 必须合法")
}

#[test]
fn json_beautify_and_minify() {
    let source = r#"{"a":1,"b":[2,3],"c":"中"}"#;
    let pretty = run_op_text("jsonBeautify", source, &[]);
    assert!(pretty.starts_with("{\n  \"a\": 1,"), "{pretty}");
    // 中文不转义：CyberChef 的 JSON Beautify 也不转，界面才能直接读
    assert!(pretty.contains("\"c\": \"中\""), "{pretty}");
    assert_eq!(
        run_op_text("jsonBeautify", r#"{"a":1}"#, &[("indent", "4")]),
        "{\n    \"a\": 1\n}"
    );
    assert_eq!(
        run_op_text("jsonMinify", "  { \"a\" : [ 1 , 2 ] }  ", &[]),
        r#"{"a":[1,2]}"#
    );
    // 空容器保持紧凑，不留下只有换行的空块
    assert_eq!(
        run_op_text("jsonBeautify", r#"{"a":{},"b":[]}"#, &[]),
        "{\n  \"a\": {},\n  \"b\": []\n}"
    );
    let message = expect_error("jsonBeautify", "{bad}", &[]);
    assert!(message.contains("不是合法 JSON"), "{message}");
}

#[test]
fn jwt_decode_shows_dates_and_verifies_hs256() {
    // header {alg:HS256,typ:JWT} / payload {sub:"1",exp:1767225600} / secret "secret"
    let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwiZXhwIjoxNzY3MjI1NjAwfQ.0Za49bLnbM4eRTnUleOo3ZDhfWg-P62Pau90iW9_LLg";
    let out = run_op_text("jwtDecode", token, &[]);
    assert!(out.contains("HS256"), "{out}");
    assert!(out.contains("2026-01-01T00:00:00Z"), "{out}");
    assert!(out.contains("expText"), "{out}");
    assert!(out.contains("签名算法：HS256"), "{out}");
    assert!(
        run_op_text("jwtDecode", token, &[("secret", "secret")]).contains("验签：通过"),
        "验签应当通过"
    );
    let wrong = run_op_text("jwtDecode", token, &[("secret", "other")]);
    assert!(wrong.contains("不通过"), "{wrong}");
    assert!(expect_error("jwtDecode", "a.b", &[]).contains("三段式"));
    assert!(expect_error("jwtDecode", "aaa.bbb.ccc", &[]).contains("Base64URL"));
}

#[test]
fn hex_dump_styles_line_up() {
    assert_eq!(
        run_op_text("hexDump", "hi!", &[("style", "plain")]),
        "686921\n\n共 3 字节"
    );
    let xxd = run_op_text("hexDump", "hello world", &[]);
    assert!(
        xxd.starts_with("00000000: 68 65 6c 6c 6f 20 77 6f  72 6c 64             hello world"),
        "{xxd}"
    );
    let hexdump = run_op_text("hexDump", "hello", &[("style", "hexdump")]);
    assert!(hexdump.contains("|hello|"), "{hexdump}");
    assert_eq!(
        run_op_text("hexDump", "abcd", &[("bytesPerLine", "2")])
            .lines()
            .count(),
        3,
        "两字节一行 × 2 + 末尾统计行"
    );
    assert!(expect_error("hexDump", "", &[]).contains("空的"));
}

#[test]
fn protobuf_wire_tree_is_readable_without_a_proto_file() {
    // 由脚本按 wire format 手工拼出来的样例：
    // 1 varint 10 / 2 "hi" / 3 message{1:"sub"} / 4 fixed64 1234 / 5 fixed32 7
    let bytes = from_hex("080a120268691a05120373756221d2040000000000002d07000000");
    let out = run_op_bytes("protobufDecode", &bytes, &[]).text_lossy();
    assert!(out.contains("1 varint 10"), "{out}");
    assert!(out.contains(r#"2 string "hi""#), "{out}");
    // 嵌套 message 要缩进展开，而不是当成一串字节
    assert!(out.contains("3 message"), "{out}");
    assert!(out.contains(r#"  2 string "sub""#), "{out}");
    assert!(out.contains("4 fixed64 1234"), "{out}");
    assert!(out.contains("5 fixed32 7"), "{out}");
    // 截断的包必须说清差多少字节
    let message = expect_error(
        "protobufDecode",
        &String::from_utf8_lossy(&bytes[..bytes.len() - 3]),
        &[],
    );
    assert!(message.contains("解析失败"), "{message}");
}

#[test]
fn der_tree_reads_sequence_integer_oid_and_text() {
    // SEQUENCE { INTEGER 1, OID 1.2.840.113549, UTF8String "算法中心" }
    let bytes = from_hex("301902010506062a864886f70d0c0ce7ae97e6b395e4b8ade5bf83");
    let out = run_op_bytes("derDecode", &bytes, &[]).text_lossy();
    assert!(out.contains("SEQUENCE"), "{out}");
    assert!(out.contains("INTEGER"), "{out}");
    assert!(out.contains("1.2.840.113549"), "{out}");
    assert!(out.contains("UTF8String"), "{out}");
    let message = expect_error("derDecode", &String::from_utf8_lossy(&bytes[..4]), &[]);
    assert!(message.contains("DER 解析失败"), "{message}");
}

#[test]
fn uuid_and_file_type() {
    assert_eq!(
        run_op_text("parseUuid", "90015098-3cd2-4fb0-d696-3f7d28e17f72", &[])
            .lines()
            .next(),
        Some("90015098-3cd2-4fb0-d696-3f7d28e17f72")
    );
    let out = run_op_text("parseUuid", "90015098-3cd2-4fb0-d696-3f7d28e17f72", &[]);
    assert!(out.contains("版本 4"), "{out}");
    assert!(out.contains("随机"), "{out}");
    assert!(expect_error("parseUuid", "nope", &[]).contains("32 个十六进制"));
    // 文件类型：ELF / PNG / ZIP(apk) / 纯文本 各一条
    assert!(
        run_op_bytes("detectFileType", b"\x7fELF\x02\x01\x01", &[])
            .text_lossy()
            .contains("ELF")
    );
    assert!(
        run_op_bytes("detectFileType", b"\x89PNG\r\n\x1a\n", &[])
            .text_lossy()
            .contains("PNG")
    );
    assert!(
        run_op_bytes("detectFileType", b"PK\x03\x04rest", &[])
            .text_lossy()
            .contains("ZIP")
    );
    assert!(
        run_op_bytes("detectFileType", b"just text here", &[])
            .text_lossy()
            .contains("像文本")
    );
    assert!(expect_error("detectFileType", "", &[]).contains("空的"));
}
