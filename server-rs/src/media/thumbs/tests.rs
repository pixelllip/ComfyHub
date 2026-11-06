//! `thumbs` 的单元测试（主体代码在 [`thumbs`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn png_with_alpha(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
    let path = dir.join(name);
    // 半透明的红方块：既覆盖"缩放"，也覆盖"透明区域铺底色"
    let mut img = image::RgbaImage::new(w, h);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([255, 0, 0, 128]);
    }
    img.save(&path).unwrap();
    path
}

#[test]
fn thumbnail_is_scaled_to_max_edge_and_never_upscaled() {
    let dir = crate::infra::testpaths::scratch("thumb");

    let big = png_with_alpha(&dir, "big.png", 1024, 512);
    let dest = dir.join("big.jpg");
    assert!(write_thumbnail(&big, &dest).unwrap());
    let out = image::open(&dest).unwrap();
    assert_eq!(
        (out.width(), out.height()),
        (512, 256),
        "最长边压到 512，比例保持"
    );

    // 小图不放大（放大只会更糊）
    let small = png_with_alpha(&dir, "small.png", 100, 80);
    let dest_small = dir.join("small.jpg");
    assert!(write_thumbnail(&small, &dest_small).unwrap());
    let out = image::open(&dest_small).unwrap();
    assert_eq!((out.width(), out.height()), (100, 80));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transparent_pixels_are_composited_on_the_dark_backdrop_not_black() {
    let dir = crate::infra::testpaths::scratch("thumb-alpha");
    let src = png_with_alpha(&dir, "alpha.png", 8, 8);
    let dest = dir.join("alpha.jpg");
    assert!(write_thumbnail(&src, &dest).unwrap());

    let out = image::open(&dest).unwrap().to_rgb8();
    let px = out.get_pixel(0, 0);
    // 半透明红叠在 #1E1E22 上 ≈ (142,15,17)；关键是**不能是纯黑 (0,0,0)**
    assert!(
        px[0] > 100 && px[1] < 60 && px[2] < 60,
        "像素={px:?}（黑块就是没铺底色）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undecodable_files_report_false_instead_of_panicking() {
    let dir = crate::infra::testpaths::scratch("thumb-bad");
    let bogus = dir.join("broken.png");
    std::fs::write(&bogus, b"not an image at all").unwrap();
    assert!(!write_thumbnail(&bogus, &dir.join("x.jpg")).unwrap());
    assert!(probe_image_size(&bogus).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn thumb_path_lives_next_to_media_dir() {
    let path = thumb_path(Path::new("D:/root/storage/media"), 42);
    assert!(
        path.ends_with("thumbs/42.jpg") || path.ends_with("thumbs\\42.jpg"),
        "{}",
        path.display()
    );
}
