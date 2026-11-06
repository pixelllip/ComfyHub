//! `reference_attach` 的单元测试：只喂临时目录里的真图，不碰 ComfyUI、不出网。

use super::*;
use crate::infra::testpaths;
use serde_json::json;

fn write_png(dir: &Path, name: &str, width: u32, height: u32) -> String {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([30, 60, 90]));
    image.save(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn refs(entries: Vec<Value>) -> Value {
    json!({ "referenceImages": entries })
}

fn entry(node: &str, file: &str, path: &str, exists: bool) -> Value {
    json!({ "nodeId": node, "file": file, "path": path, "exists": exists })
}

#[test]
fn 大图缩到_1024_长边且正文写明原始分辨率() {
    let root = testpaths::scratch("ref-attach-big");
    let path = write_png(&root, "ref.png", 1600, 900);
    let result = refs(vec![entry("42", "ref.png", &path, true)]);

    let mut budget = Budget::default();
    let mut already = HashSet::new();
    let batch = plan(&result, true, &mut budget, &mut already).expect("能看图就该附上");
    assert_eq!(batch.images.len(), 1);
    assert_eq!(batch.images[0].mime_type, "image/jpeg");
    assert!(
        batch.images[0].base64.starts_with("/9j/"),
        "base64 应该是一张 JPEG"
    );
    assert!(batch.text.contains("原图 1600×900"), "{}", batch.text);
    assert!(batch.text.contains("1024×576"), "{}", batch.text);
    assert!(batch.text.contains("等比缩图"), "{}", batch.text);
    assert_eq!(budget.attached(), 1);

    // 同一张路径不重复占上下文（模型可能拿不同参数再 load 一次）
    assert!(plan(&result, true, &mut budget, &mut already).is_none());
    testpaths::cleanup(&root);
}

#[test]
fn 小图原尺寸附上并照写分辨率() {
    let root = testpaths::scratch("ref-attach-small");
    let path = write_png(&root, "small.png", 320, 200);
    let result = refs(vec![entry("7", "small.png", &path, true)]);
    let batch = plan(&result, true, &mut Budget::default(), &mut HashSet::new()).unwrap();
    assert_eq!(batch.images.len(), 1);
    assert!(batch.text.contains("原图 320×200"), "{}", batch.text);
    assert!(batch.text.contains("原尺寸附上"), "{}", batch.text);
    testpaths::cleanup(&root);
}

#[test]
fn 模型不收图或协议不支持就一张都不发() {
    let root = testpaths::scratch("ref-attach-capability");
    let path = write_png(&root, "ref.png", 64, 64);
    let result = refs(vec![entry("1", "ref.png", &path, true)]);
    assert!(plan(&result, false, &mut Budget::default(), &mut HashSet::new()).is_none());
    // 判据本身：模型声明了图片 + 适配器实现了 inline_base64 才叫"能收图"
    assert!(supports_images(true, "openai-completions"));
    assert!(supports_images(true, "anthropic-messages"));
    assert!(!supports_images(false, "openai-completions"));
    assert!(!supports_images(true, "unknown-wire"));
    testpaths::cleanup(&root);
}

#[test]
fn 文件不在时跳过但要写进没附上的那一句() {
    let root = testpaths::scratch("ref-attach-missing");
    let path = write_png(&root, "ok.png", 64, 64);
    let result = refs(vec![
        entry("1", "gone.png", "", false),
        entry("2", "ok.png", &path, true),
    ]);
    let batch = plan(&result, true, &mut Budget::default(), &mut HashSet::new()).unwrap();
    assert_eq!(batch.images.len(), 1);
    assert!(batch.text.contains("没附上的"), "{}", batch.text);
    assert!(batch.text.contains("gone.png"), "{}", batch.text);
    testpaths::cleanup(&root);
}

#[test]
fn 每批最多四张_每轮最多六张() {
    let root = testpaths::scratch("ref-attach-budget");
    let mut entries: Vec<Value> = Vec::new();
    for index in 0..5 {
        let path = write_png(&root, &format!("r{index}.png"), 80, 60);
        entries.push(entry(
            &index.to_string(),
            &format!("r{index}.png"),
            &path,
            true,
        ));
    }
    let result = refs(entries);
    let mut budget = Budget::default();
    let mut already = HashSet::new();
    let first = plan(&result, true, &mut budget, &mut already).unwrap();
    assert_eq!(
        first.images.len(),
        MAX_PER_RESULT,
        "单次结果最多 {MAX_PER_RESULT} 张"
    );
    assert_eq!(budget.attached(), MAX_PER_RESULT);

    // 换一批新图：这次 Run 的总额度只剩两张
    let mut more: Vec<Value> = Vec::new();
    for index in 5..9 {
        let path = write_png(&root, &format!("r{index}.png"), 80, 60);
        more.push(entry(
            &index.to_string(),
            &format!("r{index}.png"),
            &path,
            true,
        ));
    }
    let second = plan(&refs(more), true, &mut budget, &mut already).unwrap();
    assert_eq!(second.images.len(), MAX_PER_RUN - MAX_PER_RESULT);
    assert_eq!(budget.attached(), MAX_PER_RUN);
    // 到顶之后一张都不给
    let third = write_png(&root, "last.png", 80, 60);
    assert!(plan(
        &refs(vec![entry("99", "last.png", &third, true)]),
        true,
        &mut budget,
        &mut already
    )
    .is_none());
    testpaths::cleanup(&root);
}
