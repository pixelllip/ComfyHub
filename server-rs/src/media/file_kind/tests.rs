//! `file_kind` 的单元测试（主体代码在 [`file_kind`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 魔数优先且逐类认得出() {
    let cases: Vec<(Vec<u8>, &str, FileKind, &str)> = vec![
        (
            vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0],
            "a.png",
            FileKind::Image,
            "image/png",
        ),
        (
            vec![0xFF, 0xD8, 0xFF, 0xE0],
            "a.jpg",
            FileKind::Image,
            "image/jpeg",
        ),
        (
            b"GIF89a....".to_vec(),
            "a.gif",
            FileKind::Image,
            "image/gif",
        ),
        (
            b"RIFF????WEBP".to_vec(),
            "a.webp",
            FileKind::Image,
            "image/webp",
        ),
        (
            vec![0x42, 0x4D, 0x00],
            "a.bmp",
            FileKind::Image,
            "image/bmp",
        ),
        (
            b"RIFF????WAVE".to_vec(),
            "a.wav",
            FileKind::Audio,
            "audio/wav",
        ),
        (b"OggS....".to_vec(), "a.ogg", FileKind::Audio, "audio/ogg"),
        (
            b"fLaC....".to_vec(),
            "a.flac",
            FileKind::Audio,
            "audio/flac",
        ),
        (b"ID3\x03".to_vec(), "a.mp3", FileKind::Audio, "audio/mpeg"),
        (
            b"%PDF-1.7".to_vec(),
            "a.pdf",
            FileKind::Document,
            "application/pdf",
        ),
    ];
    for (head, name, kind, mime) in cases {
        let detected = detect(&head, name, None);
        assert_eq!(detected.kind, kind, "{name}");
        assert_eq!(detected.mime_type.as_deref(), Some(mime), "{name}");
        assert!(!detected.blocked(), "{name} 不该被阻断");
    }
}

#[test]
fn 扩展名与声明_mime_骗不过签名() {
    // 声称是 png、内容是 PDF → 按签名判成文档（判定权在后端）
    let detected = detect(b"%PDF-1.7", "evil.png", Some("image/png"));
    assert_eq!(detected.kind, FileKind::Document);
    assert_eq!(detected.basis, "signature");

    // 声明是图片、内容是随机二进制 → UNKNOWN，必须阻断
    let detected = detect(&[0x00, 0x01, 0x02, 0x03, 0xFF], "x.png", Some("image/png"));
    assert_eq!(detected.kind, FileKind::Unknown);
    assert!(detected.blocked());
    assert_eq!(detected.mime_type, None);
    assert_eq!(detected.basis, "none");
}

#[test]
fn iso_bmff_按品牌分图片与视频() {
    let mut mp4 = vec![0, 0, 0, 0x20];
    mp4.extend_from_slice(b"ftypisom");
    mp4.extend_from_slice(&[0; 4]);
    let detected = detect(&mp4, "a.mp4", None);
    assert_eq!(detected.kind, FileKind::Video);
    assert_eq!(detected.mime_type.as_deref(), Some("video/mp4"));
    assert_eq!(detected.basis, "signature:ftyp");

    let mut avif = vec![0, 0, 0, 0x20];
    avif.extend_from_slice(b"ftypavif");
    avif.extend_from_slice(&[0; 4]);
    let detected = detect(&avif, "a.avif", None);
    assert_eq!(detected.kind, FileKind::Image);
    assert_eq!(detected.mime_type.as_deref(), Some("image/avif"));
    // 图片没有走 signatur:ftyp 之外的规则
    assert_eq!(detected.basis, "signature:ftyp");
}

#[test]
fn 纯文本要扩展名与内容同时成立() {
    let detected = detect("hello, world\n".as_bytes(), "notes.txt", None);
    assert_eq!(detected.kind, FileKind::Text);
    assert_eq!(detected.basis, "text-sniff");
    assert_eq!(detected.mime_type.as_deref(), Some("text/plain"));

    // 扩展名不在白名单：文本内容也不算数（就是认不出来）
    let detected = detect("hello".as_bytes(), "notes.xyz", Some("text/plain"));
    assert_eq!(detected.kind, FileKind::Unknown);

    // 含 NUL 字节 → 不是文本
    let detected = detect(&[b'h', b'i', 0, 1], "notes.txt", None);
    assert_eq!(detected.kind, FileKind::Unknown);

    // json / csv 各自给对 MIME
    assert_eq!(
        detect(b"{\"a\":1}", "x.json", None).mime_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        detect(b"a,b\n1,2", "x.csv", None).mime_type.as_deref(),
        Some("text/csv")
    );
}

#[test]
fn zip_系按扩展名给_ooxml_mime() {
    let head = [0x50, 0x4B, 0x03, 0x04, 0, 0];
    assert_eq!(
        detect(&head, "a.docx", None).mime_type.as_deref(),
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
    );
    assert_eq!(
        detect(&head, "a.zip", None).mime_type.as_deref(),
        Some("application/zip")
    );
}

#[test]
fn 空文件头与短文件头都不许崩() {
    assert_eq!(detect(&[], "a.png", None).kind, FileKind::Unknown);
    assert_eq!(detect(&[0x89], "a.png", None).kind, FileKind::Unknown);
    assert_eq!(detect(&[0, 0, 0], "a.mp4", None).kind, FileKind::Unknown);
}
