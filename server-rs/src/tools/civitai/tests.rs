//! `civitai` 的单元测试（主体代码在 [`civitai`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

// -----------------------------------------------------------------------
//  纯函数
// -----------------------------------------------------------------------

fn model_json() -> Map<String, Value> {
    serde_json::from_str::<Value>(
        r#"{"id":42,"name":"水墨风 LoRA","type":"LORA","nsfw":false,
            "creator":{"username":"someone"},
            "stats":{"downloadCount":1234,"thumbsUpCount":56},
            "description":"<p>把照片转成<b>水墨画</b>风格，适合风景与人物。</p>",
            "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":["inkwash","chinese painting"]}]}"#,
    )
    .unwrap()
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn 简介去标签压空白截断() {
    assert_eq!(strip_tags("<p>把照片转成<b>水墨画</b>风格</p>"), " 把照片转成 水墨画 风格 ");
    assert_eq!(collapse_spaces(&strip_tags("<p>a</p>")), "a");
    // `[^>]` 含换行：跨行的标签照样被吃掉
    assert_eq!(strip_tags("a<b\nc>d"), "a d");
    // `+` 至少一个字符：`<>` 不是标签
    assert_eq!(strip_tags("<>"), "<>");

    let m = to_model(&model_json()).unwrap();
    assert_eq!(m.description, "把照片转成 水墨画 风格，适合风景与人物。");
    assert!(!m.description.contains("<b>"));
    let long = "字".repeat(MAX_DESCRIPTION_CHARS + 10);
    let obj: Map<String, Value> = serde_json::from_value(json!({
        "id": 1, "name": "n", "description": long,
    }))
    .unwrap();
    assert_eq!(to_model(&obj).unwrap().description.chars().count(), MAX_DESCRIPTION_CHARS);
}

#[test]
fn 字段取值与底模兜底() {
    let m = to_model(&model_json()).unwrap();
    assert_eq!(m.id, 42);
    assert_eq!(m.name, "水墨风 LoRA");
    assert_eq!(m.base_model, "SDXL 1.0");
    assert_eq!(m.trained_words, vec!["inkwash", "chinese painting"]);
    assert_eq!(m.creator.as_deref(), Some("someone"));
    assert_eq!(m.downloads, 1234);
    assert_eq!(m.thumbs_up, 56);
    assert_eq!(m.page_url, "https://civitai.com/models/42");
    assert!(!m.nsfw);
    assert_eq!(m.size_bytes, None);
    assert_eq!(m.size_label(), "未知（作者没给文件体积）");

    // `modelVersions` 缺失时用顶层 `baseModels` **数组**兜底（它是多底模语义）
    let fallback: Map<String, Value> = serde_json::from_value(json!({
        "id": 7, "name": "只有顶层底模", "type": "LORA",
        "baseModels": ["Illustrious", "SDXL 1.0"],
    }))
    .unwrap();
    assert_eq!(to_model(&fallback).unwrap().base_model, "Illustrious");

    // 认不出 id 就丢掉这一条（不是给一条 id=0 的假数据）
    let no_id: Map<String, Value> = serde_json::from_value(json!({"name": "x"})).unwrap();
    assert!(to_model(&no_id).is_none());
    // 名字缺失时用 `#id` 占位（Kotlin `ifBlank { "#$id" }`）
    let no_name: Map<String, Value> = serde_json::from_value(json!({"id": 9})).unwrap();
    assert_eq!(to_model(&no_name).unwrap().name, "#9");
}

/// 体积分档（用户建议 ②）：小的用 MB、大的用 GB，**拿不到就说不知道**。
#[test]
fn 体积标签按量级分档() {
    let with = |bytes: i64| CivitaiModel {
        id: 1,
        name: "n".into(),
        r#type: "LORA".into(),
        base_model: String::new(),
        nsfw: false,
        creator: None,
        downloads: 0,
        thumbs_up: 0,
        page_url: "u".into(),
        description: String::new(),
        trained_words: vec![],
        size_bytes: Some(bytes),
        download_url: None,
        file_name: None,
        sha256: None,
    };
    assert_eq!(with(14_145_495 * 1024).size_label(), "13.49 GB");
    assert_eq!(with(300 * 1024 * 1024).size_label(), "300 MB");
    assert_eq!(
        CivitaiModel { size_bytes: None, ..with(1) }.size_label(),
        "未知（作者没给文件体积）"
    );
}

/// sha256 只认 `hashes.SHA256` 且**必须是 64 位十六进制** —— `AutoV2` 那种短码
/// 拿去校验盘上的文件永远对不上，报它比不报更糟。
#[test]
fn sha256_只认_64_位十六进制() {
    let hashes = |value: Value| value.as_object().cloned();
    let sha = "c".repeat(64);
    assert_eq!(
        primary_sha256(hashes(json!({"SHA256": sha, "AutoV2": "ABCDEF1234"})).as_ref()),
        Some(sha.clone())
    );
    assert_eq!(
        primary_sha256(hashes(json!({"sha256": sha.clone()})).as_ref()),
        Some(sha),
        "键名大小写不敏感"
    );
    assert_eq!(primary_sha256(hashes(json!({"AutoV2": "ABCDEF1234"})).as_ref()), None);
    assert_eq!(
        primary_sha256(hashes(json!({"SHA256": "ABCDEF1234"})).as_ref()),
        None,
        "长度不对就不是 sha256"
    );
    assert_eq!(primary_sha256(None), None);
}

#[test]
fn 渲染的抬头与条目形状() {
    let m = to_model(&model_json()).unwrap();
    let text = render(&[m.clone()], "水墨风", false, true);
    assert!(text.starts_with("Civitai 搜索「水墨风」：1 条"), "{text}");
    assert!(!text.contains("匿名通道"), "安全档不许出现匿名通道那句：{text}");
    assert!(text.contains("[1] 水墨风 LoRA（LORA · 底模 SDXL 1.0）"), "{text}");
    assert!(text.contains("用途：把照片转成 水墨画 风格，适合风景与人物。"), "{text}");
    assert!(text.contains("触发词：inkwash, chinese painting"), "{text}");
    assert!(!text.contains("体积："), "拿不到体积就不写这一行：{text}");
    assert!(text.contains("https://civitai.com/models/42（下载 1234 · 赞 56）"), "{text}");

    // NSFW 且没令牌 → 抬头补一句"本次是匿名通道"（放宽的是能不能搜，不是要不要说实话）
    let nsfw_text = render(&[m.clone()], "水墨风", true, true);
    assert!(nsfw_text.contains("（NSFW 通道）"), "{nsfw_text}");
    assert!(nsfw_text.contains("本次没带 Civitai 令牌，走的是匿名通道"), "{nsfw_text}");
    // 有令牌时不该出现那句
    assert!(!render(&[m], "水墨风", true, false).contains("匿名通道"));
    // 一条都没有 → 空串（调用方按 NO_RESULTS 处理）
    assert_eq!(render(&[], "x", false, true), "");
}

/// 结构化结果（用户建议 ⑦："登记在数据库中"）：可空键**省掉**而不是写 null。
#[test]
fn 结构化结果的键取舍() {
    let m = to_model(&model_json()).unwrap();
    let outcome = CivitaiOutcome {
        query: "水墨风".into(),
        models: vec![m.clone()],
        context: String::new(),
        meta: json!({}),
    };
    let value = outcome.to_json();
    assert_eq!(value["query"], "水墨风");
    assert_eq!(value["count"], 1);
    let entry = &value["models"][0];
    assert_eq!(entry["id"], 42);
    assert_eq!(entry["name"], "水墨风 LoRA");
    assert_eq!(entry["pageUrl"], "https://civitai.com/models/42");
    assert!(entry["description"].as_str().unwrap().contains("把照片转成"));
    assert!(entry["trainedWords"].as_array().unwrap().iter().any(|w| w == "inkwash"));
    let keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
    // 这几样上游这条没给 → **整个键不出现**（写 null / 0 会被误读成"很小""校验过了"）
    for absent in ["sizeBytes", "sizeLabel", "downloadUrl", "fileName", "sha256"] {
        assert!(!keys.iter().any(|k| k.as_str() == absent), "拿不到就不写这个键：{absent} → {entry}");
    }
    assert_eq!(entry["creator"], "someone", "作者给了就照写");

    // 有了 creator / 体积 / 直链 / sha256 才写那几个键，顺序也照 Kotlin
    let mut full = m;
    full.creator = Some("someone".into());
    full.size_bytes = Some(14_145_495 * 1024);
    full.download_url = Some("https://civitai.com/api/download/models/9".into());
    full.file_name = Some("krea2.safetensors".into());
    full.sha256 = Some("c".repeat(64));
    let value = CivitaiOutcome { query: "q".into(), models: vec![full], context: String::new(), meta: json!({}) }.to_json();
    let entry = &value["models"][0];
    let keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "id", "name", "type", "baseModel", "nsfw", "creator", "downloads", "thumbsUp",
            "pageUrl", "description", "sizeBytes", "sizeLabel", "downloadUrl", "fileName",
            "sha256", "trainedWords",
        ]
    );
    assert_eq!(entry["sizeLabel"], "13.49 GB", "结构化字段与正文用同一个标签");
}

#[test]
fn 客户端不许收空地址也不许留空令牌() {
    assert!(CivitaiClient::new("  ", SAFE_BASE_URL, 5, None).is_err());
    assert!(CivitaiClient::new(SAFE_BASE_URL, " ", 5, None).is_err());
    let client = CivitaiClient::new("https://civitai.com/", NSFW_BASE_URL, 999, Some("  ".into())).unwrap();
    assert_eq!(client.safe_base_url(), "https://civitai.com", "结尾斜杠要去掉");
    assert!(!client.has_token(), "空白令牌等于没配");
    assert!(CivitaiClient::new(SAFE_BASE_URL, NSFW_BASE_URL, 5, Some("t".into())).unwrap().has_token());
}

// -----------------------------------------------------------------------
//  真 socket（进程内假 Civitai，**不出网**）
// -----------------------------------------------------------------------

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State as AxumState};
use axum::response::{IntoResponse, Response};
use axum::Router;

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    query: String,
    authorization: Option<String>,
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    status: u16,
    body: String,
}

/// 两个"域名"都指向同一台本机假服务：**判据是客户端选了哪个 base**。
async fn fake_civitai(status: u16, body: &str) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake { seen: seen.clone(), status, body: body.to_string() };
    let app = Router::new().fallback(axum::routing::any(
        |AxumState(f): AxumState<Fake>, request: Request| async move {
            let (parts, body) = request.into_parts();
            let _ = axum::body::to_bytes(body, 1024 * 1024).await;
            f.seen.lock().unwrap().push(Seen {
                path: parts.uri.path().to_string(),
                query: parts.uri.query().unwrap_or_default().to_string(),
                authorization: parts
                    .headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            });
            Response::builder()
                .status(f.status)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(f.body.clone()))
                .unwrap()
                .into_response()
        },
    ))
    .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定假 Civitai");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("假 Civitai 服务");
    });
    std::mem::forget(handle);
    (format!("http://{addr}"), seen)
}

const DEFAULT_BODY: &str = r#"{"items":[{"id":42,"name":"水墨风 LoRA","type":"LORA","nsfw":false,
  "creator":{"username":"someone"},
  "stats":{"downloadCount":1234,"thumbsUpCount":56},
  "description":"<p>把照片转成<b>水墨画</b>风格，适合风景与人物。</p>",
  "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":["inkwash","chinese painting"]}]}]}"#;

fn client(base: &str, token: Option<&str>) -> CivitaiClient {
    CivitaiClient::new(base, base, 10, token.map(str::to_string)).expect("客户端")
}

/// 安全档：不带令牌、**不写 nsfw 参数**，但查询串该有的都有。
#[tokio::test]
async fn 安全搜索不带令牌也不写_nsfw_参数() {
    let (base, seen) = fake_civitai(200, DEFAULT_BODY).await;
    let outcome = client(&base, None).search("水墨风", false, None, 8).await.unwrap();

    let hit = seen.lock().unwrap()[0].clone();
    assert_eq!(hit.path, "/api/v1/models");
    assert_eq!(hit.authorization, None, "安全档不该带令牌");
    assert!(!hit.query.contains("nsfw"), "没要成人内容就不要写 nsfw 参数：{}", hit.query);
    assert!(hit.query.contains("query=%E6%B0%B4%E5%A2%A8%E9%A3%8E"), "{}", hit.query);
    assert!(hit.query.contains("&limit=8"), "{}", hit.query);
    // `Most Downloaded` 里的空格**必须**编码（Kotlin 侧为此吃过一次 URISyntaxException）
    assert!(hit.query.contains("&sort=Most+Downloaded"), "{}", hit.query);
    assert!(hit.query.contains("&period=AllTime"), "{}", hit.query);
    assert!(!hit.query.contains("types="), "没指定类型就不写这个参数：{}", hit.query);

    assert_eq!(outcome.models.len(), 1);
    assert_eq!(outcome.meta["host"], "127.0.0.1");
}

/// NSFW 档：**域名真的切到 `.red`**、明写 `nsfw=true`、有令牌就带上。
///
/// 假服务把两个"域名"指向两台不同的本机服务 —— 判据是客户端选了哪个 base，
/// 所以这组断言能分辨安全档与成人档（而不是"都打了同一个地方"）。
#[tokio::test]
async fn nsfw_搜索走成人站_base_并明写_nsfw_参数() {
    let (safe, safe_seen) = fake_civitai(200, DEFAULT_BODY).await;
    let (nsfw, nsfw_seen) = fake_civitai(200, DEFAULT_BODY).await;
    let client = CivitaiClient::new(&safe, &nsfw, 10, Some("civ_test".into())).unwrap();
    let outcome = client.search("任意查询", true, None, 8).await.unwrap();

    assert!(safe_seen.lock().unwrap().is_empty(), "成人档不许打安全站");
    let hit = nsfw_seen.lock().unwrap()[0].clone();
    assert_eq!(hit.authorization.as_deref(), Some("Bearer civ_test"), "成人档必须带令牌");
    assert!(hit.query.contains("nsfw=true"), "成人档要明写 nsfw=true：{}", hit.query);
    assert_eq!(outcome.meta["nsfw"], true);
    assert_eq!(outcome.meta["tokenless"], false);
}

/// 没配令牌时的 NSFW 搜索（2026-09-20 用户决定放宽）：**照搜**，
/// 但正文里如实写明这次是匿名通道 —— 放宽的是"能不能搜"，没有放宽"要不要说实话"。
#[tokio::test]
async fn nsfw_没令牌也照搜并在正文里写明匿名通道() {
    let (safe, _) = fake_civitai(200, DEFAULT_BODY).await;
    let (nsfw, nsfw_seen) = fake_civitai(200, DEFAULT_BODY).await;
    let client = CivitaiClient::new(&safe, &nsfw, 10, None).unwrap();
    let outcome = client.search("任意查询", true, None, 8).await.unwrap();

    let hit = nsfw_seen.lock().unwrap()[0].clone();
    assert_eq!(hit.authorization, None, "没配令牌就不带 Authorization 头");
    assert!(hit.query.contains("nsfw=true"), "{}", hit.query);
    assert_eq!(outcome.meta["nsfw"], true);
    assert_eq!(outcome.meta["tokenless"], true);
    assert!(outcome.context.contains("匿名通道"), "要如实说明这次没带令牌：{}", outcome.context);

    // 安全档的正文里不许出现那句（那是 NSFW 档的提示，别到处贴）
    let plain = client.search("水墨风", false, None, 8).await.unwrap();
    assert!(!plain.context.contains("匿名通道"), "{}", plain.context);
    assert!(!plain.meta["nsfw"].as_bool().unwrap());
}

/// `type` 归一成官方拼写；`limit` 夹到上限；**认不出的类型当场报错、不发请求**。
#[tokio::test]
async fn type_归一_limit_有上限_写错当场报错() {
    let (base, seen) = fake_civitai(200, DEFAULT_BODY).await;
    let client = client(&base, None);
    client.search("q", false, Some("lora"), 999).await.unwrap();
    let query = seen.lock().unwrap()[0].query.clone();
    assert!(query.contains("types=LORA"), "小写要归一成官方拼写：{query}");
    assert!(query.contains("&limit=20"), "limit 必须被夹到上限：{query}");

    let before = seen.lock().unwrap().len();
    let err = client.search("q", false, Some("Loraa"), 8).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("LORA"), "要把可选值列出来：{}", err.message);
    assert!(err.message.contains("Loraa"), "{}", err.message);
    assert_eq!(seen.lock().unwrap().len(), before, "本地就能判错的东西不该出网");

    // 空白查询词当场报 INVALID_ARGUMENT（也不出网）
    let err = client.search("   ", false, None, 8).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert_eq!(seen.lock().unwrap().len(), before);
}

/// 错误映射与令牌保密：401 → `CIVITAI_AUTH`、429 → `CIVITAI_RATE_LIMIT`，
/// **两处的正文里都不许出现令牌**（AIH-051）。
#[tokio::test]
async fn 错误映射与令牌保密() {
    let (base, _) = fake_civitai(401, r#"{"message":"unauthorized"}"#).await;
    let err = client(&base, Some("super-secret-token")).search("q", false, None, 8).await.unwrap_err();
    assert_eq!(err.code, "CIVITAI_AUTH");
    assert!(!err.message.contains("super-secret-token"), "错误里不能出现令牌：{}", err.message);

    let (base, _) = fake_civitai(429, r#"{"error":"rate limited"}"#).await;
    let err = client(&base, None).search("q", false, None, 8).await.unwrap_err();
    assert_eq!(err.code, "CIVITAI_RATE_LIMIT");
    assert!(err.message.contains("429"), "{}", err.message);

    for (status, expected) in [(403_u16, "CIVITAI_AUTH"), (503, "CIVITAI_FAILED"), (418, "CIVITAI_FAILED")] {
        let (base, _) = fake_civitai(status, "{}").await;
        let err = client(&base, None).search("q", false, None, 8).await.unwrap_err();
        assert_eq!(err.code, expected, "status={status}");
    }
    // 2xx 但不是 JSON 对象
    let (base, _) = fake_civitai(200, "<html>nope</html>").await;
    let err = client(&base, None).search("q", false, None, 8).await.unwrap_err();
    assert_eq!(err.code, "CIVITAI_FAILED");
    assert!(err.message.contains("无法解析"), "{}", err.message);
}

/// 体积与直链、以及 sha256 **一路进到正文与结构化字段**。
#[tokio::test]
async fn 体积直链与_sha256_进正文与结构化字段() {
    let sha = "c".repeat(64);
    let body = format!(
        r#"{{"items":[{{"id":9,"name":"krea2 Turbo Int8","type":"Checkpoint","nsfw":false,
          "stats":{{"downloadCount":10,"thumbsUpCount":2}},
          "description":"大底模",
          "modelVersions":[{{"baseModel":"Flux.1 D","trainedWords":[],
            "files":[{{"name":"krea2.safetensors","primary":true,"sizeKB":14145495.0,
                      "hashes":{{"SHA256":"{sha}","AutoV2":"ABCDEF1234"}},
                      "downloadUrl":"https://civitai.com/api/download/models/9"}}]}}]}}]}}"#
    );
    let (base, _) = fake_civitai(200, &body).await;
    let outcome = client(&base, None).search("krea2 turbo", false, None, 8).await.unwrap();
    let m = &outcome.models[0];
    assert_eq!(m.size_label(), "13.49 GB");
    assert_eq!(m.sha256.as_deref(), Some(sha.as_str()));
    assert_eq!(m.file_name.as_deref(), Some("krea2.safetensors"));
    assert_eq!(m.download_url.as_deref(), Some("https://civitai.com/api/download/models/9"));
    assert!(outcome.context.contains("体积：13.49 GB"), "{}", outcome.context);
    assert!(outcome.context.contains("直链：https://civitai.com/api/download/models/9"), "{}", outcome.context);
    let value = outcome.to_json();
    assert_eq!(value["models"][0]["sizeLabel"], "13.49 GB");
    assert_eq!(value["models"][0]["sha256"], sha);
    assert_eq!(value["models"][0]["fileName"], "krea2.safetensors");

    // 作者没给体积 → 正文不写这一行、结构化字段不写这两个键（不写 0 / null）
    let (base, _) = fake_civitai(200, DEFAULT_BODY).await;
    let plain = client(&base, None).search("水墨风", false, None, 8).await.unwrap();
    assert!(!plain.context.contains("体积："), "{}", plain.context);
    assert!(plain.to_json()["models"][0].get("sizeLabel").is_none());
}

/// 一条都搜不到 → 空结果（工具层按 `NO_RESULTS` 处理），不是"返回坏了"。
#[tokio::test]
async fn 空结果不是错误() {
    let (base, _) = fake_civitai(200, r#"{"items":[]}"#).await;
    let outcome = client(&base, None).search("查无此模", false, None, 8).await.unwrap();
    assert!(outcome.is_empty());
    assert_eq!(outcome.context, "");
    assert_eq!(outcome.meta["count"], 0);
}
