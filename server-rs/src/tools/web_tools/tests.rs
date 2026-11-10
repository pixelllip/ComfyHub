//! `web_tools` 的单元测试（主体代码在 [`web_tools`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

// -----------------------------------------------------------------------
//  纯函数
// -----------------------------------------------------------------------

/// 编码是 **Java `URLEncoder`** 的口径：空格变 `+`（不是 `%20`）、`*` 保留。
///
/// 这一条错了**不会报错**：`url=https%3A%2F%2F…` 那一段依然"看起来对"，
/// 只是有些目标抓不回来。
#[test]
fn 编码是_java_urlencoder_的口径() {
    assert_eq!(url_encode("Most Downloaded"), "Most+Downloaded");
    assert_eq!(url_encode("a b"), "a+b");
    assert_eq!(
        url_encode("https://example.com/page?id=42&lang=zh"),
        "https%3A%2F%2Fexample.com%2Fpage%3Fid%3D42%26lang%3Dzh"
    );
    assert_eq!(url_encode("水墨风"), "%E6%B0%B4%E5%A2%A8%E9%A3%8E");
    // `-_.*` 是 Java 明确保留的那几个
    assert_eq!(
        url_encode("krea2_turbo-int8.safetensors*"),
        "krea2_turbo-int8.safetensors*"
    );
    // 大写十六进制（小写会被某些服务端拒）
    assert_eq!(url_encode("~"), "%7E");
}

#[test]
fn 空白压平与_java_trim_都是_ascii_口径() {
    assert_eq!(
        crate::tools::tools::collapse_spaces("  把照片转成\n\n <b>水墨画</b> 风格  "),
        "把照片转成 <b>水墨画</b> 风格"
    );
    assert_eq!(crate::tools::tools::collapse_spaces("\t\n"), "");
    // Java 的 `String.trim()` 只去 `<= U+0020`：全角空格**不去**（`\s` 也不认它）
    assert_eq!(crate::tools::tools::java_trim("  x\u{3000}  "), "x\u{3000}");
}

#[test]
fn 正文截断按_utf16_单元数() {
    assert_eq!(utf16_len("水墨风"), 3);
    assert_eq!(
        utf16_len("🙂"),
        2,
        "emoji 在 Kotlin 的 String.length 里算 2"
    );
    assert_eq!(take_utf16("水墨风", 2), "水墨");
    // 不能把一个代理对切成半个字符
    assert_eq!(take_utf16("a🙂b", 2), "a");
    assert_eq!(take_utf16("abc", 9), "abc");
}

#[test]
fn 服务端错误正文按_message_error_detail_取且截到_300() {
    assert_eq!(detail_of(r#"{"message":"先看 message"}"#), "先看 message");
    assert_eq!(detail_of(r#"{"error":"再看 error"}"#), "再看 error");
    assert_eq!(detail_of(r#"{"detail":"最后 detail"}"#), "最后 detail");
    assert_eq!(
        detail_of(r#"{"message":"","error":"空的不算"}"#),
        "空的不算"
    );
    // 一个都没有 → 空串（调用方据此省掉"｜返回："那一段）
    assert_eq!(detail_of(r#"{"ok":true}"#), "");
    // 解析不出来 → 原样截断
    assert_eq!(detail_of("boom"), "boom");
    assert_eq!(detail_of(&"字".repeat(400)).chars().count(), 300);
}

/// 渲染是**按整条取舍**的：超预算的那条整个跳过，编号不许留空洞。
#[test]
fn 渲染按整条取舍且编号无空洞() {
    let results = vec![
        WebResult {
            title: "标题一".into(),
            url: "https://a/1".into(),
            snippet: "片段一".into(),
            published_at: Some("2026-08-01".into()),
        },
        WebResult {
            title: "".into(),
            url: "https://a/2".into(),
            snippet: " 多  空白\n片段 ".into(),
            published_at: None,
        },
    ];
    let text = render(&results);
    assert_eq!(
        text,
        "[1] 标题一 (https://a/1) - 发布 2026-08-01\n片段一\n\n[2] (无标题) (https://a/2)\n多 空白 片段"
    );

    // 塞一条超预算的进去：它被跳过，后面的编号接着数（[2] 而不是 [3]）
    let huge = WebResult {
        title: "巨大".into(),
        url: "https://a/huge".into(),
        snippet: "字".repeat(CONTEXT_MAX_CHARS),
        published_at: None,
    };
    let skipped = render(&[results[0].clone(), huge, results[1].clone()]);
    assert!(skipped.contains("[2] (无标题) (https://a/2)"), "{skipped}");
    assert!(!skipped.contains("巨大"), "超预算的那条整个不给：{skipped}");

    // 第一条自己也超预算时**照样留着**（`kept.isEmpty()` 那一档，与 Kotlin 同）
    let only = render(&[WebResult {
        title: "长".into(),
        url: "u".into(),
        snippet: "字".repeat(CONTEXT_MAX_CHARS * 2),
        published_at: None,
    }]);
    assert!(only.starts_with("[1] 长 (u)"));
}

/// `web_fetch` 的三档 URL 报错与 Kotlin 逐字一致（模型靠它自己改对写法）。
#[test]
fn url_三档判定的文案与_kotlin_一致() {
    assert_eq!(
        split_http_url("http://a b/x").unwrap_err().code,
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        split_http_url("http://a b/x").unwrap_err().message,
        "url 无法解析：http://a b/x"
    );
    assert_eq!(
        split_http_url("ftp://example.com/x").unwrap_err().message,
        "url 必须是 http/https 链接"
    );
    assert_eq!(
        split_http_url("notaurl").unwrap_err().message,
        "url 必须是 http/https 链接"
    );
    assert_eq!(
        split_http_url("http:///path").unwrap_err().message,
        "url 里没有主机名：http:///path"
    );
    assert_eq!(
        split_http_url("http:/path").unwrap_err().message,
        "url 里没有主机名：http:/path"
    );
    assert_eq!(
        split_http_url("http://user:pass@Example.COM:8080/a?b#c").unwrap(),
        ("http".to_string(), "example.com".to_string()),
        "user:pass@ 与端口都要去掉，主机名小写"
    );
    assert_eq!(
        split_http_url("https://[::1]:8188/x").unwrap().1,
        "[::1]",
        "IPv6 保留方括号（Java URI 同）"
    );
    assert_eq!(
        split_http_url("https://Example.COM/a").unwrap(),
        ("https".to_string(), "example.com".to_string())
    );
}

/// 内网 / 回环 / 云元数据**一律拒绝**（这是 `web_fetch` 唯一的安全边界）。
#[test]
fn 取正文的目标不许是内网与本机() {
    for host in [
        "127.0.0.1",
        "localhost",
        "192.168.1.1",
        "169.254.169.254",
        "metadata.google.internal",
        "0.0.0.0",
        "[::1]",
    ] {
        let err = guard_public_host(host).unwrap_err();
        assert_eq!(err.code, "FETCH_TARGET_DENIED", "{host}");
        assert!(
            err.message.contains("只允许公开网络地址"),
            "{host}: {}",
            err.message
        );
    }
    assert!(guard_public_host("example.com").is_ok());
    assert!(guard_public_host("8.8.8.8").is_ok());
}

/// 搜索服务**自己**的地址按推断档判：回环放行（本机假服务 / 自建代理），
/// 云元数据仍然拒（那条红线任何档位都不放）。
#[test]
fn 搜索服务地址按推断档判() {
    let local = KeenableClient::new("http://127.0.0.1:9/api", CLIENT_SOURCE, 5, None).unwrap();
    assert_eq!(local.base_url(), "http://127.0.0.1:9/api", "末尾斜杠要去掉");
    assert!(local.guard_base_host("127.0.0.1").is_ok());
    assert!(local.guard_base_host("api.keenable.ai").is_ok());
    let err = local.guard_base_host("169.254.169.254").unwrap_err();
    assert_eq!(err.code, "SEARCH_FAILED");
    assert!(
        err.message.contains("搜索服务地址不被允许"),
        "{}",
        err.message
    );
    // 空地址：本机的 bug，如实报（Kotlin 那边会抛一个非 ToolFailure 的异常）
    assert!(KeenableClient::new("   ", CLIENT_SOURCE, 5, None).is_err());
}

// -----------------------------------------------------------------------
//  真 socket（进程内假 Keenable，**不出网**）
// -----------------------------------------------------------------------

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State as AxumState};
use axum::response::{IntoResponse, Response};
use axum::Router;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    title: Option<String>,
    api_key: Option<String>,
    body: String,
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    status: u16,
    body: String,
}

/// 起一台进程内的假 Keenable，返回 `(base, 记录)`。
async fn fake_keenable(status: u16, body: &str) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake {
        seen: seen.clone(),
        status,
        body: body.to_string(),
    };
    let app = Router::new()
        .fallback(axum::routing::any(
            |AxumState(f): AxumState<Fake>, request: Request| async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
                    .await
                    .unwrap_or_default();
                f.seen.lock().unwrap().push(Seen {
                    method: parts.method.to_string(),
                    path: parts.uri.path().to_string(),
                    query: parts.uri.query().unwrap_or_default().to_string(),
                    title: parts
                        .headers
                        .get("x-keenable-title")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    api_key: parts
                        .headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    body: String::from_utf8_lossy(&bytes).to_string(),
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 Keenable");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("假 Keenable 服务");
    });
    std::mem::forget(handle);
    (format!("http://{addr}"), seen)
}

const SEARCH_BODY: &str = r#"{"query":"flux 提示词技巧","mode":"pro","results":[
  {"title":"Flux 提示词指南","url":"https://example.com/flux","snippet":"正文片段一","published_at":"2026-08-01"},
  {"title":"第二条","url":"https://example.com/2","snippet":"片段二"}]}"#;
// ⚠ 正文里有 `"#`（markdown 的 `# 全文`），所以裸字符串要用 `r##"…"##` 收尾
const FETCH_BODY: &str =
    r##"{"url":"https://example.com/flux","title":"Flux 指南","content":"# 全文\n正文内容"}"##;

fn client(base: &str, key: Option<&str>) -> KeenableClient {
    KeenableClient::new(base, CLIENT_SOURCE, 10, key.map(str::to_string)).expect("客户端")
}

/// 免密钥：路径必须带 `/public`、必须带 `X-Keenable-Title`、**不许**带 `X-API-Key`。
#[tokio::test]
async fn 免密钥搜索打_public_并带标题头() {
    let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
    let outcome = client(&base, None)
        .search("flux 提示词技巧", None, None, None, None)
        .await
        .unwrap();

    let hits = seen.lock().unwrap().clone();
    assert_eq!(hits.len(), 1, "一次搜索只该发一个请求：{hits:?}");
    let hit = &hits[0];
    assert_eq!(hit.method, "POST");
    assert_eq!(hit.path, "/v1/search/public", "免密钥必须走 /public");
    assert_eq!(
        hit.title.as_deref(),
        Some(CLIENT_SOURCE),
        "缺这个头会被服务端 400"
    );
    assert_eq!(hit.api_key, None, "免密钥请求不该带 X-API-Key");
    assert!(hit.body.contains("\"query\""), "{}", hit.body);
    assert!(hit.body.contains("flux"), "{}", hit.body);
    assert!(
        hit.body.contains("\"mode\":\"pro\""),
        "mode 默认 pro：{}",
        hit.body
    );
    assert!(
        hit.body.contains("\"snippet_max_length\":2500"),
        "{}",
        hit.body
    );
    assert!(!hit.body.contains("site"), "没给的键不许出现：{}", hit.body);

    assert_eq!(outcome.results.len(), 2);
    assert!(
        outcome
            .context
            .contains("[1] Flux 提示词指南 (https://example.com/flux)"),
        "{}",
        outcome.context
    );
    assert!(
        outcome.context.contains("- 发布 2026-08-01"),
        "{}",
        outcome.context
    );
    assert!(
        outcome.context.contains("正文片段一"),
        "{}",
        outcome.context
    );
    assert_eq!(outcome.meta["keyless"], true);
    assert_eq!(outcome.meta["count"], 2);
    assert_eq!(outcome.meta["query"], "flux 提示词技巧");
    assert!(!outcome.is_empty());
}

/// 配了 key：走 `/v1/search`（**不带 `/public`**）并带 `X-API-Key`。
#[tokio::test]
async fn 带密钥走非_public_路径() {
    let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
    let outcome = client(&base, Some("keen_test"))
        .search("任意查询", None, None, None, None)
        .await
        .unwrap();
    let hit = seen.lock().unwrap()[0].clone();
    assert_eq!(hit.path, "/v1/search", "带 key 时不能走 /public");
    assert_eq!(hit.api_key.as_deref(), Some("keen_test"));
    assert_eq!(outcome.meta["keyless"], false);
}

/// 可选参数（site / 日期）只在给了的时候进 body。
#[tokio::test]
async fn 可选参数只在给了的时候进_body() {
    let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
    client(&base, None)
        .search(
            "q",
            Some("arxiv.org"),
            Some("2026-01-01"),
            Some("2026-02-01"),
            None,
        )
        .await
        .unwrap();
    let body = seen.lock().unwrap()[0].body.clone();
    assert!(body.contains("\"site\":\"arxiv.org\""), "{body}");
    assert!(
        body.contains("\"published_after\":\"2026-01-01\""),
        "{body}"
    );
    assert!(
        body.contains("\"published_before\":\"2026-02-01\""),
        "{body}"
    );
}

/// 429：免密钥说清额度与出路；带 key 说清是密钥额度 —— **两处都绝不回显密钥**。
#[tokio::test]
async fn 限流的两种文案且绝不回显密钥() {
    let (base, _) = fake_keenable(429, r#"{"error":"quota"}"#).await;
    let err = client(&base, None)
        .search("q", None, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SEARCH_RATE_LIMIT");
    assert!(
        err.message.contains("1000"),
        "要说明免密钥额度：{}",
        err.message
    );
    assert!(
        err.message.contains("｜返回：quota"),
        "服务端的 message 要带上：{}",
        err.message
    );

    let (base, _) = fake_keenable(429, r#"{"message":"too many"}"#).await;
    let err = client(&base, Some("keen_secret_value"))
        .search("q", None, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SEARCH_RATE_LIMIT");
    assert!(
        err.message.contains("额度"),
        "带 key 时要说清是密钥额度用尽：{}",
        err.message
    );
    assert!(
        !err.message.contains("keen_secret_value"),
        "绝不回显密钥：{}",
        err.message
    );
}

/// 其余状态码各自映射到稳定码（模型按码分支，不是按那句话）。
#[tokio::test]
async fn 状态码映射到稳定错误码() {
    for (status, expected) in [
        (401_u16, "SEARCH_AUTH"),
        (403, "SEARCH_AUTH"),
        (402, "SEARCH_QUOTA"),
        (400, "SEARCH_BAD_REQUEST"),
        (503, "SEARCH_FAILED"),
        (418, "SEARCH_FAILED"),
    ] {
        let (base, _) = fake_keenable(status, r#"{"detail":"why"}"#).await;
        let err = client(&base, None)
            .search("q", None, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, expected, "status={status}: {}", err.message);
    }
    // 2xx 但正文不是 JSON 对象
    for body in ["<html>Just a moment...</html>", "[]", "{}"] {
        let (base, _) = fake_keenable(200, body).await;
        let outcome = client(&base, None)
            .search("q", None, None, None, None)
            .await;
        if body == "{}" {
            // 空对象是合法的（就是"一条都没有"）—— 调用方按 NO_RESULTS 处理
            assert!(outcome.unwrap().is_empty());
        } else {
            let err = outcome.unwrap_err();
            assert_eq!(err.code, "SEARCH_FAILED", "{body}");
            assert!(err.message.contains("无法解析"), "{body}: {}", err.message);
        }
    }
}

/// `web_fetch`：GET `/v1/fetch/public`，目标 URL 编进查询串（`url=` 那一段是钉住的）。
#[tokio::test]
async fn 取正文把目标_url_编码进查询串() {
    let (base, seen) = fake_keenable(200, FETCH_BODY).await;
    let page = client(&base, None)
        .page("https://example.com/page?id=42&lang=zh", 20_000)
        .await
        .unwrap();
    let hit = seen.lock().unwrap()[0].clone();
    assert_eq!(hit.path, "/v1/fetch/public");
    assert_eq!(hit.method, "GET");
    assert_eq!(hit.title.as_deref(), Some(CLIENT_SOURCE));
    assert!(
        hit.query
            .starts_with("url=https%3A%2F%2Fexample.com%2Fpage%3Fid%3D42%26lang%3Dzh"),
        "目标 URL 必须整段编码：{}",
        hit.query
    );
    assert_eq!(page.title, "Flux 指南");
    assert!(page.content.contains("正文内容"), "{}", page.content);
    assert!(!page.truncated);
    assert_eq!(
        page.url, "https://example.com/flux",
        "服务端给了归一后的 url 就用它"
    );
}

/// 正文超上限：切到上限并**如实标 truncated**（不许假装给全了）。
#[tokio::test]
async fn 取正文超上限时截断并标出来() {
    let long = "字".repeat(100);
    let body = format!(r#"{{"url":"https://example.com/x","title":"t","content":"{long}"}}"#);
    let (base, _) = fake_keenable(200, &body).await;
    let page = client(&base, None)
        .page("https://example.com/x", 10)
        .await
        .unwrap();
    assert_eq!(page.content.chars().count(), 10);
    assert!(page.truncated);
    // 上限被夹到 [1, 60000]
    let (base, _) = fake_keenable(200, &body).await;
    let page = client(&base, None)
        .page("https://example.com/x", 0)
        .await
        .unwrap();
    assert_eq!(page.content.chars().count(), 1, "下限是 1");
}

/// **被拒的目标一个请求都不发**（这条是 SSRF 防线的回归）。
#[tokio::test]
async fn 内网目标在出网之前就被拒() {
    let (base, seen) = fake_keenable(200, FETCH_BODY).await;
    let client = client(&base, None);
    for target in [
        "http://127.0.0.1:9222/x",
        "http://192.168.1.1/x",
        "http://169.254.169.254/latest/meta-data",
        "http://localhost:8188/object_info",
    ] {
        let err = client.page(target, 1000).await.unwrap_err();
        assert_eq!(err.code, "FETCH_TARGET_DENIED", "{target}");
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "被拒的目标一个请求都不该发出去"
    );
}

/// 连不上：**本机最常见的形态**是连接被立刻拒（不是超时）—— 文案要说清"该配代理"。
#[tokio::test]
async fn 连不上时报出出网路径而不是假装没搜到() {
    // 127.0.0.1 上一个没有服务的端口：连接被立刻拒绝
    let err = client("http://127.0.0.1:1", None)
        .search("q", None, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SEARCH_FAILED");
    assert!(err.message.contains("连不上搜索服务"), "{}", err.message);
    assert!(
        err.message.contains("本次出网路径"),
        "要写清本次是直连还是代理：{}",
        err.message
    );
    assert!(
        !err.message.contains("没有搜到"),
        "不许把'没查成'说成'没搜到'：{}",
        err.message
    );
}
