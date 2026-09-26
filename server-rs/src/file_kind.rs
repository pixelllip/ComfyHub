//! 严格文件类型识别（AIH-027 / RSK-008）—— Kotlin `FileKindDetector` 的逐条移植。
//!
//! # 为什么不复用 `media_upload::kind_of()`
//!
//! 那个函数的兜底是"未知即 IMAGE"，用在画廊里只是显示不准，用在**模型输入**上就是
//! **把未知文件当图片发给上游**。这里反过来：**签名优先，认不出来就是 `Unknown`，
//! `Unknown` 一律阻断**（[`Detected::blocked`]）。
//!
//! 扩展名和声明 MIME 只作为辅助（纯文本这类没有魔数），**绝不单独作为放行依据**。

/// 识别出的类型（`wire` 就是界面 / 库里那个小写字）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Document,
    Text,
    Unknown,
}

impl FileKind {
    /// 库里 / 界面上的字（Kotlin `FileKind.name.lowercase()`）。
    pub fn wire(&self) -> &'static str {
        match self {
            FileKind::Image => "image",
            FileKind::Video => "video",
            FileKind::Audio => "audio",
            FileKind::Document => "document",
            FileKind::Text => "text",
            FileKind::Unknown => "unknown",
        }
    }

    /// 对应的输入模态（`Unknown` 没有）。
    pub fn modality(&self) -> Option<&'static str> {
        match self {
            FileKind::Image => Some("image"),
            FileKind::Video => Some("video"),
            FileKind::Audio => Some("audio"),
            FileKind::Document => Some("document"),
            FileKind::Text => Some("text"),
            FileKind::Unknown => None,
        }
    }
}

/// 一次识别的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub kind: FileKind,
    /// 探测出的 MIME；`Unknown` 时为 `None`
    pub mime_type: Option<String>,
    /// 判断依据：`signature` / `signature:ftyp` / `text-sniff` / `none`（审计与排错用）
    pub basis: String,
}

impl Detected {
    /// 认不出来一律阻断（绝不做"未知即图片"的乐观回退）。
    pub fn blocked(&self) -> bool {
        self.kind == FileKind::Unknown
    }
}

/// 纯文本扩展名白名单：这些类型没有魔数，只能靠内容嗅探 + 扩展名共同确认。
const TEXT_EXT: [&str; 9] = ["txt", "md", "markdown", "csv", "tsv", "json", "yaml", "yml", "log"];

/// 识别一个文件（`head` 是开头若干字节，建议 ≥ 64B）。
pub fn detect(head: &[u8], file_name: &str, declared_mime: Option<&str>) -> Detected {
    let ext: String = file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default()
        .chars()
        .take(12)
        .collect();

    // 1) 魔数优先
    if let Some(detected) = signature(head, &ext) {
        return detected;
    }

    // 2) ISO BMFF（mp4/mov/avif/heic…）：ftyp 盒子
    if let Some(mime) = ftyp_mime(head) {
        let kind = if mime.starts_with("video/") { FileKind::Video } else { FileKind::Image };
        return Detected { kind, mime_type: Some(mime), basis: "signature:ftyp".to_string() };
    }

    // 3) 纯文本：内容全是可打印 UTF-8 且扩展名在白名单里才算数
    if !head.is_empty() && TEXT_EXT.contains(&ext.as_str()) && looks_like_text(head) {
        return Detected {
            kind: FileKind::Text,
            mime_type: Some(text_mime(&ext).to_string()),
            basis: "text-sniff".to_string(),
        };
    }

    // 4) 声明 MIME 是已知的文本类，且扩展名也在白名单 → 仍然只当文本
    if TEXT_EXT.contains(&ext.as_str())
        && declared_mime.map(|m| m.starts_with("text/")).unwrap_or(false)
        && looks_like_text(head)
    {
        return Detected {
            kind: FileKind::Text,
            mime_type: Some(text_mime(&ext).to_string()),
            basis: "text-sniff".to_string(),
        };
    }

    // 5) 认不出来 —— 不认识就是不认识，不做任何乐观回退
    Detected { kind: FileKind::Unknown, mime_type: None, basis: "none".to_string() }
}

fn signature(bytes: &[u8], ext: &str) -> Option<Detected> {
    fn at(b: &[u8], i: usize, expected: &[u8]) -> bool {
        b.len() >= i + expected.len() && &b[i..i + expected.len()] == expected
    }
    fn ascii(b: &[u8], i: usize, text: &str) -> bool {
        at(b, i, text.as_bytes())
    }
    let image = |mime: &str| Some(Detected {
        kind: FileKind::Image,
        mime_type: Some(mime.to_string()),
        basis: "signature".to_string(),
    });
    let audio = |mime: &str| Some(Detected {
        kind: FileKind::Audio,
        mime_type: Some(mime.to_string()),
        basis: "signature".to_string(),
    });

    if at(bytes, 0, &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return image("image/png");
    }
    if at(bytes, 0, &[0xFF, 0xD8, 0xFF]) {
        return image("image/jpeg");
    }
    if ascii(bytes, 0, "GIF87a") || ascii(bytes, 0, "GIF89a") {
        return image("image/gif");
    }
    if ascii(bytes, 0, "RIFF") && ascii(bytes, 8, "WEBP") {
        return image("image/webp");
    }
    if at(bytes, 0, &[0x42, 0x4D]) {
        return image("image/bmp");
    }
    if at(bytes, 0, &[0x49, 0x49, 0x2A, 0x00]) || at(bytes, 0, &[0x4D, 0x4D, 0x00, 0x2A]) {
        return image("image/tiff");
    }

    // 音视频容器
    if ascii(bytes, 0, "RIFF") && ascii(bytes, 8, "WAVE") {
        return audio("audio/wav");
    }
    if at(bytes, 0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(Detected {
            kind: FileKind::Video,
            mime_type: Some("video/webm".to_string()),
            basis: "signature:ebml".to_string(),
        });
    }
    if ascii(bytes, 0, "OggS") {
        return audio("audio/ogg");
    }
    if ascii(bytes, 0, "fLaC") {
        return audio("audio/flac");
    }
    if ascii(bytes, 0, "ID3")
        || at(bytes, 0, &[0xFF, 0xFB])
        || at(bytes, 0, &[0xFF, 0xF3])
        || at(bytes, 0, &[0xFF, 0xF2])
    {
        return audio("audio/mpeg");
    }

    // 文档
    if ascii(bytes, 0, "%PDF-") {
        return Some(Detected {
            kind: FileKind::Document,
            mime_type: Some("application/pdf".to_string()),
            basis: "signature".to_string(),
        });
    }
    if at(bytes, 0, &[0x50, 0x4B, 0x03, 0x04]) {
        return Some(Detected {
            kind: FileKind::Document,
            mime_type: Some(ooxml_mime(ext).to_string()),
            basis: "signature:zip".to_string(),
        });
    }
    None
}

/// ISO BMFF：`....ftyp<brand>`，用品牌的 MIME 前缀区分图片 / 视频。
fn ftyp_mime(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 12 {
        return None;
    }
    if &bytes[4..8] != b"ftyp" {
        return None;
    }
    let brand = String::from_utf8_lossy(&bytes[8..12]).trim().to_lowercase();
    let mime = match brand.as_str() {
        "avif" | "avis" => "image/avif",
        "heic" | "heix" | "hevc" | "mif1" => "image/heic",
        "qt" => "video/quicktime",
        "isom" | "iso2" | "mp41" | "mp42" | "avc1" | "dash" | "m4v" | "msnv" => "video/mp4",
        _ => return None,
    };
    Some(mime.to_string())
}

fn ooxml_mime(ext: &str) -> &'static str {
    match ext {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "epub" => "application/epub+zip",
        _ => "application/zip",
    }
}

fn text_mime(ext: &str) -> &'static str {
    match ext {
        "json" => "application/json",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        "yaml" | "yml" => "application/yaml",
        _ => "text/plain",
    }
}

/// 内容嗅探：没有 NUL 字节、且能按 UTF-8 解码。
///
/// 只看开头一段即可 —— 攻击者可以构造"开头像文本、后面是二进制"的文件，
/// 所以这类文件最终也只会被判为 `Text`，而不会被当成图片放行。
fn looks_like_text(head: &[u8]) -> bool {
    !head.contains(&0) && std::str::from_utf8(head).is_ok()
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 魔数优先且逐类认得出() {
        let cases: Vec<(Vec<u8>, &str, FileKind, &str)> = vec![
            (vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0], "a.png", FileKind::Image, "image/png"),
            (vec![0xFF, 0xD8, 0xFF, 0xE0], "a.jpg", FileKind::Image, "image/jpeg"),
            (b"GIF89a....".to_vec(), "a.gif", FileKind::Image, "image/gif"),
            (b"RIFF????WEBP".to_vec(), "a.webp", FileKind::Image, "image/webp"),
            (vec![0x42, 0x4D, 0x00], "a.bmp", FileKind::Image, "image/bmp"),
            (b"RIFF????WAVE".to_vec(), "a.wav", FileKind::Audio, "audio/wav"),
            (b"OggS....".to_vec(), "a.ogg", FileKind::Audio, "audio/ogg"),
            (b"fLaC....".to_vec(), "a.flac", FileKind::Audio, "audio/flac"),
            (b"ID3\x03".to_vec(), "a.mp3", FileKind::Audio, "audio/mpeg"),
            (b"%PDF-1.7".to_vec(), "a.pdf", FileKind::Document, "application/pdf"),
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
        assert_eq!(detect(b"{\"a\":1}", "x.json", None).mime_type.as_deref(), Some("application/json"));
        assert_eq!(detect(b"a,b\n1,2", "x.csv", None).mime_type.as_deref(), Some("text/csv"));
    }

    #[test]
    fn zip_系按扩展名给_ooxml_mime() {
        let head = [0x50, 0x4B, 0x03, 0x04, 0, 0];
        assert_eq!(
            detect(&head, "a.docx", None).mime_type.as_deref(),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(detect(&head, "a.zip", None).mime_type.as_deref(), Some("application/zip"));
    }

    #[test]
    fn 空文件头与短文件头都不许崩() {
        assert_eq!(detect(&[], "a.png", None).kind, FileKind::Unknown);
        assert_eq!(detect(&[0x89], "a.png", None).kind, FileKind::Unknown);
        assert_eq!(detect(&[0, 0, 0], "a.mp4", None).kind, FileKind::Unknown);
    }
}
