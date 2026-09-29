//! 缩略图（P4 第 5 步）—— 替掉 Kotlin 的 `ImageIO` + TwelveMonkeys(WebP)。
//!
//! # 与 Kotlin `MediaFiles.writeThumbnail` 逐条对齐的四个点
//!
//! | 点 | 值 / 做法 | 为什么 |
//! | --- | --- | --- |
//! | 最长边 | **512** | 与 Kotlin 同值；前端按这个尺寸铺瀑布流 |
//! | 缩放 | `scale = min(1.0, 512 / max(w,h))`，**不放大** | 小图放大只会更糊、还更占空间 |
//! | 底色 | 先铺 `#1E1E22` 再把图叠上去 | 透明 PNG 转 JPEG 会变**黑块**（深灰底看起来才是"没有内容"） |
//! | 质量 | JPEG **0.82** | 与 `compressionQuality = 0.82f` 同值 |
//!
//! # 失败即回退（不是报错）
//!
//! 解码不了（AVIF 之类没有解码器的格式、或者文件其实是坏的）→ 调用方**回退到原图**，
//! 与 Kotlin 路由里的那条分支一致：宁可给用户一张大图，也不要一个破图/500。
//!
//! 非图片（视频/音频）→ **204 No Content**（前端据此不放缩略图）——同样照抄 Kotlin。

use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, Rgb};

/// 缩略图最长边（与 Kotlin 同值 512）。
pub const MAX_EDGE: u32 = 512;

/// 透明区域的底色（与 Kotlin 的 `Color(0x1E,0x1E,0x22)` 同值）。
const BACKDROP: Rgb<u8> = Rgb([0x1E, 0x1E, 0x22]);

/// JPEG 质量（与 Kotlin 的 `compressionQuality = 0.82f` 同值）。
const QUALITY: u8 = 82;

/// 缩略图路径：`<storage>/thumbs/<mediaId>.jpg`（与 Kotlin `Storage.thumbPath` 同布局——
/// **两种实现必须写同一个位置**，否则灰度期切来切去会让缩略图反复重建/互相覆盖）。
pub fn thumb_path(media_dir: &Path, media_id: i64) -> PathBuf {
    media_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("thumbs")
        .join(format!("{media_id}.jpg"))
}

/// 生成缩略图。返回 `Ok(true)` = 写成功；`Ok(false)` = **解码不了**（调用方回退原图）。
pub fn write_thumbnail(source: &Path, dest: &Path) -> std::io::Result<bool> {
    let decoded = match image::open(source) {
        Ok(img) => img,
        Err(_) => return Ok(false), // 没有解码器 / 文件坏了 → 回退原图
    };
    let (w0, h0) = (decoded.width(), decoded.height());
    if w0 == 0 || h0 == 0 {
        return Ok(false);
    }

    let scale = (MAX_EDGE as f64 / w0.max(h0) as f64).min(1.0);
    let w = ((w0 as f64 * scale) as u32).max(1);
    let h = ((h0 as f64 * scale) as u32).max(1);

    // 先铺底色再叠图：透明 PNG → JPEG 不出现黑块。
    // 画布用 **RGBA**（`imageops::overlay` 要求两张图同像素类型，且它做的是 alpha 混合），
    // 叠完再转 RGB8 交给 JPEG 编码器（JPEG 没有 alpha 通道）。
    let mut canvas = image::RgbaImage::from_pixel(
        w,
        h,
        image::Rgba([BACKDROP[0], BACKDROP[1], BACKDROP[2], 255]),
    );
    let resized = decoded.resize_exact(w, h, image::imageops::FilterType::Triangle).to_rgba8();
    image::imageops::overlay(&mut canvas, &resized, 0, 0);
    let canvas = DynamicImage::ImageRgba8(canvas).to_rgb8();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(dest)?;
    let mut encoder = JpegEncoder::new_with_quality(&mut file, QUALITY);
    encoder
        .encode_image(&DynamicImage::ImageRgb8(canvas))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    Ok(true)
}

/// 图片的原生尺寸（上传入库时要存 `width`/`height`；与 Kotlin `probeImageSize` 同用途）。
pub fn probe_image_size(source: &Path) -> Option<(i64, i64)> {
    let decoded = image::open(source).ok()?;
    Some((decoded.width() as i64, decoded.height() as i64))
}

#[cfg(test)]
mod tests {
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
        let dir = crate::testpaths::scratch("thumb");

        let big = png_with_alpha(&dir, "big.png", 1024, 512);
        let dest = dir.join("big.jpg");
        assert!(write_thumbnail(&big, &dest).unwrap());
        let out = image::open(&dest).unwrap();
        assert_eq!((out.width(), out.height()), (512, 256), "最长边压到 512，比例保持");

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
        let dir = crate::testpaths::scratch("thumb-alpha");
        let src = png_with_alpha(&dir, "alpha.png", 8, 8);
        let dest = dir.join("alpha.jpg");
        assert!(write_thumbnail(&src, &dest).unwrap());

        let out = image::open(&dest).unwrap().to_rgb8();
        let px = out.get_pixel(0, 0);
        // 半透明红叠在 #1E1E22 上 ≈ (142,15,17)；关键是**不能是纯黑 (0,0,0)**
        assert!(px[0] > 100 && px[1] < 60 && px[2] < 60, "像素={px:?}（黑块就是没铺底色）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undecodable_files_report_false_instead_of_panicking() {
        let dir = crate::testpaths::scratch("thumb-bad");
        let bogus = dir.join("broken.png");
        std::fs::write(&bogus, b"not an image at all").unwrap();
        assert_eq!(write_thumbnail(&bogus, &dir.join("x.jpg")).unwrap(), false);
        assert!(probe_image_size(&bogus).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumb_path_lives_next_to_media_dir() {
        let path = thumb_path(Path::new("D:/root/storage/media"), 42);
        assert!(path.ends_with("thumbs/42.jpg") || path.ends_with("thumbs\\42.jpg"), "{}", path.display());
    }
}
