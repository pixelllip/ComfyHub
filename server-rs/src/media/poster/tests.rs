//! `poster` 的单元测试（主体代码在 [`poster`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 封面路径与缩略图分开存() {
    let media_dir = PathBuf::from(r"D:\proj\storage\media");
    assert_eq!(
        poster_path(&media_dir, 42),
        PathBuf::from(r"D:\proj\storage\thumbs\42.poster.png")
    );
    // 图片缩略图是 `.jpg`（`thumbs.rs`），两者互不覆盖
    assert_eq!(
        crate::media::thumbs::thumb_path(&media_dir, 42),
        PathBuf::from(r"D:\proj\storage\thumbs\42.jpg")
    );
}

#[test]
fn 找不到脚本时如实回_false_而不是假装成功() {
    let dir = crate::infra::testpaths::scratch("poster");
    let missing_root = dir.join("nothing-here");
    std::fs::create_dir_all(&missing_root).unwrap();
    assert!(locate_poster_script(&missing_root).is_none());
    let dest = dir.join("out.png");
    assert!(!write_video_poster(
        &missing_root,
        &dir.join("a.mp4"),
        &dest,
        POSTER_SIZE
    ));
    assert!(!dest.exists(), "失败时不许留半份文件");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 脚本位置按项目根拼() {
    let root = PathBuf::from(r"D:\proj");
    assert_eq!(
        root.join(POSTER_SCRIPT),
        PathBuf::from(r"D:\proj\scripts\video-poster.ps1")
    );
}
