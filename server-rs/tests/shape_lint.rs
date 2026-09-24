//! **形状防线（源码扫描）**：Kotlin 镜像 DTO 里的可空字段**不许吐 `null`**。
//!
//! # 这条防线的由来（2026-09-24，真库副本实测）
//!
//! Kotlin 的 `AppJson` 是 `explicitNulls = false`（`server/src/main/kotlin/com/comfyhub/Json.kt:12`），
//! 所以**可空字段为 null 时整键不出现**。同一份真库副本上把两个实现对着比：
//!
//! ```text
//! Kotlin 8090: {"id":18,"name":"ComfyUI","useCount":44,"createdAt":"…"}
//! Rust   8091: {"id":18,"name":"ComfyUI","normalized":"comfyui","category":null,
//!               "color":null,"description":null,"useCount":44,"createdAt":"…"}
//! ```
//!
//! 而 `/api/prompts` 上更明显：27 条里 24 条的形状不一致（`notes` / `negativePrompt` /
//! `batchSize` / `cfgScale` / `width` / `height` / `sampler` / `scheduler` / `sourceRef` …）。
//! 形状错**不报错**，只是"多一个键"或"界面少一块内容"——`api_test.rs` 的键集合断言能抓住已有的，
//! 但抓不住**新加的 DTO**（下一个就是 `/api/ai/models`，它有一堆
//! `thinkingFormat` / `maxOutputTokens` / `capabilityVerifiedAt` 这样的可空字段）。
//!
//! 所以这里做一条源码扫描：**任何派生了 `Serialize` 的 `pub struct`，字段类型是 `Option<…>`
//! 就必须带 `skip_serializing_if`**（例外见 [`ALLOWED_NULLS`]，只有一条且写明理由）。
//!
//! 为什么用源码扫描而不是逐个 DTO 构造一遍再断言：新 DTO 是**别人下一轮加的**，
//! 只有"扫全仓"才可能在你没想到的地方拦住它。

use std::path::{Path, PathBuf};

/// 唯一的豁免：`sync.rs` 的 `kit_version`。
///
/// 同步 API 是 Rust 侧新定的（Kotlin 没有对应端点），契约 §1 与 `api_test` 都写死了
/// "没写过就回 null"（手机据此判断桌面还没有工件版本）。**它不是 Kotlin 镜像 DTO**，
/// 所以不适用"省略键"那条规则。
const ALLOWED_NULLS: [(&str, &str); 1] = [("sync.rs", "kit_version")];

/// 这个 struct 的字段里，哪些 `Option<…>` 没带 `skip_serializing_if`。
///
/// 返回 `(行号, 字段名)`。只扫 `#[derive(… Serialize …)]` 的 `pub struct`。
fn option_fields_without_skip(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("pub struct ") {
            i += 1;
            continue;
        }
        // 往回看属性块：里面有 `Serialize` 才算"会序列化出去的结构"
        let mut serializes = false;
        let mut j = i;
        while j > 0 {
            j -= 1;
            let prev = lines[j].trim();
            if prev.is_empty() {
                continue; // 允许属性块与 struct 之间有空行
            }
            if prev.starts_with("#[") {
                if prev.contains("Serialize") {
                    serializes = true;
                }
                continue;
            }
            break; // 撞到上一个结构的 `}` 或文档注释就停
        }
        if !serializes {
            i += 1;
            continue;
        }

        // 扫结构体体内，直到顶格的 `}`
        let mut pending_skip = false;
        let mut k = i + 1;
        while k < lines.len() {
            let line = lines[k].trim();
            if line == "}" {
                break;
            }
            if line.starts_with("#[") {
                if line.contains("skip_serializing_if") {
                    pending_skip = true;
                }
                k += 1;
                continue;
            }
            if let Some(field) = line.strip_prefix("pub ") {
                let compact: String = field.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.contains(":Option<") && !pending_skip {
                    let name = field.split(':').next().unwrap_or("?").trim().to_string();
                    out.push((k + 1, name));
                }
                pending_skip = false;
            } else if !line.is_empty() && !line.starts_with("//") {
                // 非字段、非注释、非属性：属性块结束（例如 `}` 收尾的嵌套类型）
                pending_skip = false;
            }
            k += 1;
        }
        i = k + 1;
    }
    out
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 全仓扫描：**可空字段一律省略键**（`explicitNulls = false` 的形状）。
#[test]
fn kotlin_mirror_dtos_never_emit_a_json_null() {
    let mut offenders: Vec<String> = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(src_dir())
        .expect("读 src/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "rs").unwrap_or(false))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "没扫到任何 .rs：路径算错了");

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(path).unwrap();
        for (line, field) in option_fields_without_skip(&src) {
            if ALLOWED_NULLS.iter().any(|(f, n)| *f == name && *n == field) {
                continue;
            }
            offenders.push(format!("{name}:{line}  `{field}`"));
        }
    }

    assert!(
        offenders.is_empty(),
        "这些可空字段会序列化成 `null`，而 Kotlin（explicitNulls = false）是**整个键不出现**。\n\
         给字段加 `#[serde(skip_serializing_if = \"Option::is_none\")]`（并照 Kotlin 的 DTO 核一遍）：\n  {}",
        offenders.join("\n  ")
    );
}

/// 扫描器自己也要有防线：拿一段"故意写错"的源码，必须被抓出来。
#[test]
fn the_scanner_actually_catches_missing_skip() {
    let bad = r#"
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fake {
    pub id: i64,
    pub missing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<String>,
    #[serde(rename = "baseURL")]
    pub also_missing: Option<String>,
}

// 没有 Serialize：不该被抓
pub struct NotSerialized {
    pub whatever: Option<String>,
}

struct Private {
    pub field: Option<String>,
}
"#;
    let found = option_fields_without_skip(bad);
    let names: Vec<String> = found.iter().map(|(_, n)| n.clone()).collect();
    assert_eq!(names, vec!["missing", "also_missing"], "扫描器漏了或误报了：{found:?}");

    // 空行分隔的属性块照样认得出 Serialize
    let ok = "#[derive(Serialize)]\n\npub struct Spaced {\n    pub a: Option<String>,\n}\n";
    assert_eq!(option_fields_without_skip(ok).len(), 1);
}
