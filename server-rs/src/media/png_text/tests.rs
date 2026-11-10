//! `png_text` 的单元测试（主体代码在 [`png_text`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn png_with(chunks: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    for (name, data) in chunks {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC 不校验
    }
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

fn temp_png(label: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = crate::infra::testpaths::scratch(&format!("png-{label}")).join("image.png");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn text_块明文读得出来() {
    let mut data = b"prompt\x00".to_vec();
    data.extend_from_slice(b"{\"1\":{\"class_type\":\"KSampler\"}}");
    let path = temp_png("text", &png_with(&[("tEXt", &data)]));
    let chunks = read_text_chunks(&path);
    assert_eq!(
        chunks.get("prompt").unwrap(),
        "{\"1\":{\"class_type\":\"KSampler\"}}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn ztxt_与_itxt_都能解() {
    // zTXt：keyword \0 method(0) + zlib 数据
    let mut ztxt = b"workflow\x00\x00".to_vec();
    ztxt.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(
        b"{\"nodes\":[]}".as_slice(),
        6,
    ));
    // iTXt：keyword \0 compFlag=0 method=0 lang \0 translated \0 text
    let mut itxt = b"prompt\x00\x00\x00en\x00note\x00".to_vec();
    itxt.extend_from_slice(b"{\"2\":{\"class_type\":\"CLIPTextEncode\"}}");
    let path = temp_png("ztxt", &png_with(&[("zTXt", &ztxt), ("iTXt", &itxt)]));
    let chunks = read_text_chunks(&path);
    assert_eq!(chunks.get("workflow").unwrap(), "{\"nodes\":[]}");
    assert_eq!(
        chunks.get("prompt").unwrap(),
        "{\"2\":{\"class_type\":\"CLIPTextEncode\"}}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn 压过的_itxt_也解得开() {
    let mut itxt = b"prompt\x00\x01\x00\x00\x00".to_vec();
    itxt.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(
        b"{\"a\":1}".as_slice(),
        6,
    ));
    let path = temp_png("itxt-z", &png_with(&[("iTXt", &itxt)]));
    assert_eq!(read_text_chunks(&path).get("prompt").unwrap(), "{\"a\":1}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn 不是_png_与畸形文件都回空表而不是崩() {
    let path = temp_png("notpng", b"not a png at all");
    assert!(read_text_chunks(&path).is_empty());
    // 签名对、块长度谎报 → 直接停（不越界读）
    let mut broken = SIGNATURE.to_vec();
    broken.extend_from_slice(&9_999_999u32.to_be_bytes());
    broken.extend_from_slice(b"tEXt");
    broken.extend_from_slice(b"xx");
    let path = temp_png("broken", &broken);
    assert!(read_text_chunks(&path).is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn 同一个键只留先到的() {
    let mut first = b"prompt\x00".to_vec();
    first.extend_from_slice(b"first");
    let mut second = b"prompt\x00".to_vec();
    second.extend_from_slice(b"second");
    let path = temp_png("dup", &png_with(&[("tEXt", &first), ("tEXt", &second)]));
    assert_eq!(read_text_chunks(&path).get("prompt").unwrap(), "first");
    let _ = std::fs::remove_file(path);
}
