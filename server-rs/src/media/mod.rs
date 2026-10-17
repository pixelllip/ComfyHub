//! `media/` —— 媒体与文件。
//!
//! 产物读写与文件流（Range/ETag）、上传、缩略图、视频封面、PNG 内嵌文本、严格类型识别、AI 附件与准入。

pub mod attachments;
pub mod collection;
pub mod file_kind;
pub mod media;
pub mod media_upload;
pub mod media_write;
pub mod png_text;
pub mod poster;
pub mod strict_intake;
pub mod thumbs;
