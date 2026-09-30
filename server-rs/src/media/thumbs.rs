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
mod tests;
