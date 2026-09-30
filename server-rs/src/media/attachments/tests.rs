//! `attachments` 的单元测试（主体代码在 [`attachments`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn state(label: &str) -> (AppState, PathBuf) {
    let dir = crate::infra::testpaths::scratch(&format!("att-{label}-{}", uuid_like()));
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path).unwrap();
    conn.execute_batch(&schema).unwrap();
    let state = AppState::new(conn, ":memory:", dir.join("media"));
    (state, dir)
}

fn png_bytes() -> Vec<u8> {
    let mut image = image::RgbImage::new(6, 4);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb([9, 200, 30]);
    }
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}

#[test]
fn 图片附件入库_尺寸与缩略图都对() {
    let (state, dir) = state("png");
    let tmp = temp_file(&state).unwrap();
    std::fs::write(&tmp, png_bytes()).unwrap();
    let dto = save(&state, "参考图.png", &tmp, Some("image/png")).unwrap();
    assert_eq!(dto.kind, "image");
    assert_eq!(dto.modality, "image");
    assert_eq!(dto.mime_type, "image/png");
    assert_eq!((dto.width, dto.height), (Some(6), Some(4)), "要探测图片尺寸");
    assert!(dto.sha256.is_some());
    assert!(dto.created_at.is_some(), "创建时间由库里给");
    assert!(dto.has_preview());
    assert!(!tmp.exists(), "临时文件要被移走");

    // 回读一次形状（键集合照 Kotlin：可空键不出现）
    let json = serde_json::to_value(&dto).unwrap();
    assert!(json.as_object().unwrap().contains_key("mimeType"));
    assert!(json.as_object().unwrap().contains_key("sizeBytes"));
    assert!(!json.as_object().unwrap().contains_key("hasPreview"), "计算属性不序列化");

    // 惰性缩略图
    let (thumb, mime) = thumbnail_of(&state, &dto.id).unwrap().expect("图片要有缩略图");
    assert!(thumb.is_file());
    assert_eq!(mime, "image/jpeg");

    // 原件能读回来做内联
    let encoded = inline_base64(&state, &dto).expect("8MB 以内要能内联");
    assert!(encoded.starts_with("iVBORw0KGgo"), "PNG 的 base64 前缀");
    assert_eq!(base64_decode(&encoded).unwrap(), std::fs::read(file_of(&state, &dto.id).unwrap().unwrap()).unwrap());

    // 删除连原件与缩略图一起清
    assert!(delete(&state, &dto.id).unwrap());
    assert!(get(&state, &dto.id).unwrap().is_none());
    assert!(!thumb.exists(), "缩略图也要删掉");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn 认不出来的类型一律拒收且不留半份() {
    let (state, dir) = state("reject");
    let tmp = temp_file(&state).unwrap();
    std::fs::write(&tmp, [0x00, 0x01, 0x02, 0x03, 0xFF]).unwrap();
    let error = save(&state, "evil.png", &tmp, Some("image/png")).expect_err("必须拒");
    assert_eq!(error.error, "UNSUPPORTED_CONTENT");
    assert!(error.human().contains("无法识别文件 evil.png"), "{}", error.human());
    assert!(!tmp.exists(), "被拒的临时文件必须删掉");
    // 空文件同样拒
    let tmp = temp_file(&state).unwrap();
    std::fs::write(&tmp, b"").unwrap();
    let error = save(&state, "empty.txt", &tmp, None).expect_err("空文件必须拒");
    assert!(error.human().contains("是空的"), "{}", error.human());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn 文本附件入库但没有预览() {
    let (state, dir) = state("text");
    let tmp = temp_file(&state).unwrap();
    std::fs::write(&tmp, b"# hello\nworld\n").unwrap();
    let dto = save(&state, "notes.md", &tmp, Some("text/markdown")).unwrap();
    assert_eq!(dto.kind, "text");
    assert_eq!(dto.mime_type, "text/markdown");
    assert!(!dto.has_preview());
    assert!(thumbnail_of(&state, &dto.id).unwrap().is_none(), "文本没有画面");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn 视频附件回_204_而不是假装抽过帧() {
    let (state, dir) = state("video");
    let tmp = temp_file(&state).unwrap();
    let mut bytes = vec![0, 0, 0, 0x20];
    bytes.extend_from_slice(b"ftypisom");
    bytes.extend_from_slice(&[0; 8]);
    std::fs::write(&tmp, &bytes).unwrap();
    let dto = save(&state, "clip.mp4", &tmp, Some("video/mp4")).unwrap();
    assert_eq!(dto.kind, "video");
    assert!(dto.has_preview(), "界面会去要封面");
    assert!(
        thumbnail_of(&state, &dto.id).unwrap().is_none(),
        "预览帧还没搬：如实回 null（端点 204），不假装"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn 按_id_批量取要保持顺序() {
    let (state, dir) = state("list");
    let mut ids = Vec::new();
    for (index, name) in ["a.md", "b.md", "c.md"].iter().enumerate() {
        let tmp = temp_file(&state).unwrap();
        std::fs::write(&tmp, format!("# doc {index}").as_bytes()).unwrap();
        ids.push(save(&state, name, &tmp, None).unwrap().id);
    }
    let reordered = vec![ids[2].clone(), ids[0].clone(), "不存在".to_string()];
    let found = list(&state, &reordered).unwrap();
    assert_eq!(found.len(), 2, "找不到的 id 静默跳过");
    assert_eq!(found[0].id, ids[2], "顺序必须跟传入一致");
    assert_eq!(found[1].id, ids[0]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn 非法_id_与目录穿越都挡住() {
    let (state, dir) = state("safe");
    // 非法 id 与"查不到"同一条路（Kotlin 同）：**不是**另一个错误
    assert!(get(&state, "../../etc/passwd").unwrap().is_none());
    assert!(stored_name(&state, "../../etc/passwd").unwrap().is_none());
    assert!(delete(&state, "../../etc/passwd").unwrap() == false);
    assert!(resolve_attachment(&state, "../media/x.png").is_none());
    assert!(resolve_attachment(&state, "sub/x.png").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn base64_编解码往返() {
    for case in [b"".to_vec(), b"a".to_vec(), b"ab".to_vec(), b"abc".to_vec(), vec![0u8, 255, 128, 7]] {
        let encoded = base64_encode(&case);
        assert_eq!(base64_decode(&encoded).unwrap(), case, "{encoded}");
    }
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    assert!(base64_decode("not base64!!").is_none());
}
