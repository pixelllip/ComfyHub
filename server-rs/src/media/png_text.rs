//! PNG 内嵌文本块（P4 第 52 步）—— Kotlin `PngMeta` 的逐条移植。
//!
//! ComfyUI 把**这一次生成用的节点图**写进 PNG 的 `tEXt` / `iTXt` 块的 `prompt` 键里
//! （界面格式在 `workflow` 键里）。"导入目录里已经生成好的产物"这条路就靠它把
//! "一张图"还原成"一条提示词 + 它的参数"，否则导入进来只是一堆没有来历的图片。
//!
//! 三种块都认（与 Kotlin 同）：`tEXt`（明文）、`zTXt`（zlib）、`iTXt`（可压缩、带语言标记）。
//! CRC 不校验（ComfyUI 写出来的都是合法的，校验只会多一次失败机会）；
//! 单个块的体积有上限（畸形文件不许把内存吃光）。

use std::collections::BTreeMap;
use std::path::Path;

const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// 单个块的体积上限（Kotlin `MAX_CHUNK`）。
const MAX_CHUNK: u32 = 96 * 1024 * 1024;

/// 读一张 PNG 里的文本块（键 → 文本）。**保持首次出现的顺序**（BTreeMap 只为可预测的调试输出；
/// 取用时只按 `prompt` / `workflow` 两个键，顺序不影响语义）。
pub fn read_text_chunks(path: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(bytes) = std::fs::read(path) else {
        return out;
    };
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return out;
    }
    let mut cursor = 8usize;
    loop {
        if cursor + 8 > bytes.len() {
            break;
        }
        let len = u32::from_be_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]);
        let name = String::from_utf8_lossy(&bytes[cursor + 4..cursor + 8]).to_string();
        cursor += 8;
        if len > MAX_CHUNK {
            break;
        }
        let len = len as usize;
        if cursor + len + 4 > bytes.len() {
            break;
        }
        let data = &bytes[cursor..cursor + len];
        cursor += len + 4; // 跳过 CRC（不校验）
        match name.as_str() {
            "tEXt" => parse_t_ext(data, &mut out),
            "zTXt" => parse_z_txt(data, &mut out),
            "iTXt" => parse_i_txt(data, &mut out),
            _ => {}
        }
        if name == "IEND" {
            break;
        }
    }
    out
}

fn index_of_zero(data: &[u8], from: usize) -> Option<usize> {
    data.iter()
        .skip(from)
        .position(|b| *b == 0)
        .map(|i| i + from)
}

fn put(out: &mut BTreeMap<String, String>, key: &str, value: String) {
    // `prompt` / `workflow` 可能同时存在 tEXt 与 iTXt：**保留先到的**
    let key = key.trim();
    if key.is_empty() || value.trim().is_empty() {
        return;
    }
    out.entry(key.to_string()).or_insert(value);
}

/// `tEXt`: keyword \0 text（ComfyUI 实际写 UTF-8）
fn parse_t_ext(data: &[u8], out: &mut BTreeMap<String, String>) {
    let Some(sep) = index_of_zero(data, 0) else {
        return;
    };
    if sep == 0 {
        return;
    }
    let key = String::from_utf8_lossy(&data[..sep]).to_string();
    let value = String::from_utf8_lossy(&data[sep + 1..]).to_string();
    put(out, &key, value);
}

/// `zTXt`: keyword \0 method(1) zlib(text)
fn parse_z_txt(data: &[u8], out: &mut BTreeMap<String, String>) {
    let Some(sep) = index_of_zero(data, 0) else {
        return;
    };
    if sep == 0 || sep + 2 > data.len() {
        return;
    }
    let key = String::from_utf8_lossy(&data[..sep]).to_string();
    if let Some(text) = inflate(&data[sep + 2..]) {
        put(out, &key, text);
    }
}

/// `iTXt`: keyword \0 compFlag(1) compMethod(1) lang \0 translated \0 text
fn parse_i_txt(data: &[u8], out: &mut BTreeMap<String, String>) {
    let Some(sep) = index_of_zero(data, 0) else {
        return;
    };
    if sep == 0 || sep + 2 > data.len() {
        return;
    }
    let key = String::from_utf8_lossy(&data[..sep]).to_string();
    let compressed = data[sep + 1] == 1;
    let mut cursor = sep + 3;
    let Some(lang_end) = index_of_zero(data, cursor) else {
        return;
    };
    cursor = lang_end + 1;
    let Some(trans_end) = index_of_zero(data, cursor) else {
        return;
    };
    cursor = trans_end + 1;
    if cursor > data.len() {
        return;
    }
    let body = &data[cursor..];
    let text = if compressed {
        inflate(body)
    } else {
        Some(String::from_utf8_lossy(body).to_string())
    };
    if let Some(text) = text {
        put(out, &key, text);
    }
}

/// zlib 解压（`zTXt` / 压缩的 `iTXt`）。
///
/// 用 `miniz_oxide`（纯 Rust，**不引 C 依赖**）：发布包要便携，
/// 不能为了一个 PNG 文本块把 zlib 的构建需求带进来。
fn inflate(data: &[u8]) -> Option<String> {
    let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(data).ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
