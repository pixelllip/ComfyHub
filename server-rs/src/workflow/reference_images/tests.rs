//! `reference_images` 的单元测试：只喂目录，不依赖本机装没装 ComfyUI。
//!
//! 这里钉住三件事：
//!  1. **真实文件能解析出路径与原始分辨率** —— 自动带图的地基；
//!  2. **`..` / 盘符一律拒绝** —— 工作流文件来自网上，越界读等于把用户机器上的文件发出去；
//!  3. **找不到就如实 `exists:false`** —— 不猜路径、不静默丢。

use super::*;
use crate::infra::testpaths;

/// 在 `dir` 下写一张 `width×height` 的 PNG（真图：`image_dimensions` 必须读得出来）。
fn write_png(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([20, 40, 60]));
    image.save(&path).unwrap();
    path
}

fn graph(value: Value) -> Map<String, Value> {
    serde_json::from_value(value).unwrap()
}

#[test]
fn load_image_解析出真实路径与原始分辨率() {
    let root = testpaths::scratch("ref-images-basic");
    let input = root.join("input");
    let output = root.join("output");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let written = write_png(&input, "image (15).png", 120, 80);

    let graph = graph(json!({
        "42": {"class_type": "LoadImage", "inputs": {"image": "image (15).png", "upload": "image"}},
        // 连线型 image：不是文件名，跳过
        "43": {"class_type": "LoadImage", "inputs": {"image": ["7", 0]}},
        // 不是 LoadImage：跳过
        "44": {"class_type": "PreviewImage", "inputs": {"images": ["42", 0]}}
    }));
    let refs = collect_in(&graph, std::slice::from_ref(&input), &[output]);
    assert_eq!(refs.len(), 1, "只该有节点 42：{refs:?}");
    let entry = &refs[0];
    assert_eq!(entry.node_id, "42");
    assert_eq!(entry.file, "image (15).png");
    assert!(entry.exists);
    assert_eq!(entry.path.as_deref(), Some(written.as_path()));
    assert_eq!((entry.width, entry.height), (Some(120), Some(80)));
    assert_eq!(entry.bytes, Some(std::fs::metadata(&written).unwrap().len()));
    let wire = entry.to_json();
    assert_eq!(wire["nodeId"], "42");
    assert_eq!(wire["exists"], true);
    assert_eq!(wire["width"], 120);
    testpaths::cleanup(&root);
}

#[test]
fn 路径穿越与盘符一律拒绝() {
    let root = testpaths::scratch("ref-images-traversal");
    let input = root.join("input");
    std::fs::create_dir_all(&input).unwrap();
    // 在 input 的**外面**放一份"机密"文件：只要解析得出去，它就会被读到
    let secret = write_png(&root, "secret.png", 10, 10);

    let graph = graph(json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": "../secret.png"}},
        "2": {"class_type": "LoadImage", "inputs": {"image": secret.to_string_lossy()}},
        "3": {"class_type": "LoadImage", "inputs": {"image": "sub/../../secret.png"}}
    }));
    let refs = collect_in(&graph, &[input], &[]);
    assert_eq!(refs.len(), 3);
    for entry in &refs {
        assert!(!entry.exists, "越界路径不许解析：{entry:?}");
        assert!(entry.path.is_none());
        assert!(entry.note.as_deref().unwrap_or_default().contains("不受支持"), "{entry:?}");
    }
    testpaths::cleanup(&root);
}

#[test]
fn 文件不在时如实报_exists_false() {
    let root = testpaths::scratch("ref-images-missing");
    let input = root.join("input");
    std::fs::create_dir_all(&input).unwrap();
    let graph = graph(json!({
        "89": {"class_type": "LoadImage", "inputs": {"image": "gone.png"}}
    }));
    let refs = collect_in(&graph, &[input], &[]);
    assert_eq!(refs.len(), 1);
    assert!(!refs[0].exists);
    assert!(refs[0].path.is_none());
    let note = refs[0].note.as_deref().unwrap_or_default();
    assert!(note.contains("input") && note.contains("gone.png"), "{note}");
    assert_eq!(refs[0].to_json()["exists"], false);
    testpaths::cleanup(&root);
}

#[test]
fn output_后缀的图去_output_目录找() {
    let root = testpaths::scratch("ref-images-output");
    let input = root.join("input");
    let output = root.join("output");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let written = write_png(&output, "prev.png", 64, 48);

    let graph = graph(json!({
        "5": {"class_type": "LoadImage", "inputs": {"image": "prev.png [output]"}}
    }));
    let refs = collect_in(&graph, &[input], &[output]);
    assert_eq!(refs.len(), 1);
    assert!(refs[0].exists, "{refs:?}");
    assert_eq!(refs[0].path.as_deref(), Some(written.as_path()));
    // `file` 是给模型看的名字：后缀要摘掉，否则它照着写进 overrides 会多一段
    assert_eq!(refs[0].file, "prev.png");
    testpaths::cleanup(&root);
}

#[test]
fn temp_后缀如实说明是临时图() {
    let root = testpaths::scratch("ref-images-temp");
    let input = root.join("input");
    std::fs::create_dir_all(&input).unwrap();
    let graph = graph(json!({
        "7": {"class_type": "LoadImage", "inputs": {"image": "clipspace/paint.png [temp]"}}
    }));
    let refs = collect_in(&graph, &[input], &[]);
    assert_eq!(refs.len(), 1);
    assert!(!refs[0].exists);
    assert!(refs[0].note.as_deref().unwrap_or_default().contains("临时"), "{refs:?}");
    testpaths::cleanup(&root);
}
