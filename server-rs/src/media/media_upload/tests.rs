//! `media_upload` 的单元测试（主体代码在 [`media_upload`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn mime_and_kind_follow_the_kotlin_table() {
    assert_eq!(
        detect_mime("a.PNG", None),
        "image/png",
        "扩展名大小写不敏感"
    );
    assert_eq!(
        detect_mime("a.bin", Some("image/webp")),
        "image/webp",
        "客户端给的优先"
    );
    assert_eq!(
        detect_mime("a.bin", Some("application/octet-stream")),
        "application/octet-stream",
        "占位 MIME 要落回扩展名判定（这里没扩展名，所以还是 octet-stream）"
    );
    assert_eq!(kind_of("a.mp4", None), "VIDEO");
    assert_eq!(kind_of("a.mp3", None), "AUDIO");
    assert_eq!(
        kind_of("a.unknown", None),
        "IMAGE",
        "认不出来按图片处理（与 Kotlin 同）"
    );
    assert_eq!(
        kind_of("x.png", Some("video/mp4")),
        "VIDEO",
        "客户端说视频就视频"
    );
    assert_eq!(extension_of("noext"), "");
    assert_eq!(extension_of("a.JPEG"), "jpeg");
}

#[test]
fn stored_names_are_unique_and_keep_the_extension() {
    let a = new_stored_name("图 像.PNG");
    let b = new_stored_name("图 像.PNG");
    assert!(a.ends_with(".png"), "{a}");
    assert_ne!(a, b, "两次落盘名不能撞（撞了会互相覆盖）");
    assert_eq!(new_stored_name("noext").len(), 32);
}
