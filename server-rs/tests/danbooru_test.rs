//! **`danbooru_lookup` 的回归防线**（真 socket + 进程内假 danbooru，**不出网**）。
//!
//! # 这一组在防什么
//!
//! 这条工具是"写角色之前先查 danbooru"那条纪律的**唯一执行者**（O8），而它的失败模式
//! 全都**看起来像另一个东西**：
//!
//! * 把"未查询成功"（429 / 5xx / 连接失败）说成"没有这个角色" → 会误认为角色不存在；
//! * 把 Cloudflare 的挑战页说成"UA 被拒"或"没有这个 tag" → 两个错误方向各指一边；
//! * `wiki_pages` 的 404（这个 tag 没有 wiki 页，**正常**）被当成错误 → 已经拿到的
//!   角色 tag / 别名一起丢掉；
//! * 模糊那一路的通配符 `*` 被 URL 编码成 `%2A` → 换装版本一条都搜不到，**不报错**；
//! * UA 写成"描述型" → 被 Cloudflare 拦（本机实测 6 种 UA 只有短的那种过）；
//! * 挑角色 tag 时并列取了最后一个（Kotlin `maxByOrNull` 取第一个）→ 选错角色，**不报错**。
//!
//! 所以这里每一条都单独钉住。假服务是**真 HTTP** 的：请求行与请求头都会被记下来断言，
//! 而不是"调了一个假函数"。

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State as AxumState};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use comfyhub_server_rs::tools::danbooru::{DanbooruClient, USER_AGENT};

/// 假 danbooru 收到的一次请求（**看请求行与请求头**，不只看返回值）。
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    query: String,
    user_agent: String,
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    exact: Value,
    fuzzy: Value,
    wiki: Option<Value>,
    related: Option<Value>,
    /// `tags.json` 的状态码（默认 200）
    tags_status: u16,
    tags_body: String,
    wiki_status: u16,
    related_status: u16,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            exact: json!([]),
            fuzzy: json!([]),
            wiki: None,
            related: None,
            tags_status: 200,
            tags_body: String::new(),
            wiki_status: 200,
            related_status: 200,
        }
    }
}

fn respond(status: u16, body: &str, content_type: &str) -> Response {
    (
        axum::http::StatusCode::from_u16(status).unwrap(),
        [(axum::http::header::CONTENT_TYPE, content_type)],
        body.to_string(),
    )
        .into_response()
}

/// 起一台进程内的假 danbooru，返回 (base_url, 记录)。
async fn fake_danbooru(fake: Fake) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = fake.seen.clone();

    let app = Router::new()
        .route(
            "/tags.json",
            get(|AxumState(f): AxumState<Fake>, headers: HeaderMap, uri: axum::http::Uri| async move {
                f.seen.lock().unwrap().push(Seen {
                    path: uri.path().to_string(),
                    query: uri.query().unwrap_or_default().to_string(),
                    user_agent: headers
                        .get("user-agent")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });
                if f.tags_status != 200 {
                    return respond(f.tags_status, &f.tags_body, "text/html; charset=utf-8");
                }
                // 精确 vs 模糊：按 `name_matches` 里有没有 `*` 分
                let query = uri.query().unwrap_or_default().to_string();
                let fuzzy = query.contains("%2A") || query.contains('*');
                let body = if fuzzy { f.fuzzy.clone() } else { f.exact.clone() };
                respond(200, &body.to_string(), "application/json")
            }),
        )
        .route(
            "/wiki_pages/{tag}",
            get(|AxumState(f): AxumState<Fake>, Path(tag): Path<String>| async move {
                f.seen.lock().unwrap().push(Seen {
                    path: format!("/wiki_pages/{tag}"),
                    query: String::new(),
                    user_agent: String::new(),
                });
                if f.wiki_status != 200 {
                    return respond(f.wiki_status, "not found", "text/plain");
                }
                match f.wiki.clone() {
                    Some(wiki) => respond(200, &wiki.to_string(), "application/json"),
                    None => respond(404, "not found", "text/plain"),
                }
            }),
        )
        .route(
            "/related_tag.json",
            get(|AxumState(f): AxumState<Fake>, uri: axum::http::Uri| async move {
                f.seen.lock().unwrap().push(Seen {
                    path: uri.path().to_string(),
                    query: uri.query().unwrap_or_default().to_string(),
                    user_agent: String::new(),
                });
                if f.related_status != 200 {
                    return respond(f.related_status, "boom", "text/plain");
                }
                match f.related.clone() {
                    Some(related) => respond(200, &related.to_string(), "application/json"),
                    None => respond(404, "not found", "text/plain"),
                }
            }),
        )
        .with_state(fake);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 danbooru");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("假 danbooru 服务");
    });
    // 让 handle 活到测试结束（进程退出时一起结束）
    std::mem::forget(handle);
    (format!("http://{addr}"), seen)
}

fn tag(name: &str, category: i64, post_count: i64) -> Value {
    json!({ "name": name, "category": category, "post_count": post_count })
}

fn client(base: &str) -> DanbooruClient {
    DanbooruClient::new(base, USER_AGENT, 10).expect("客户端")
}

#[tokio::test]
async fn 查角色要拿齐角色tag别名换装版本与同现tag() {
    let fake = Fake {
        exact: json!([tag("shinku_(nte)", 4, 100), tag("shinku", 0, 900)]),
        fuzzy: json!([
            tag("shinku_(nte)", 4, 100),
            tag("shinku_(dawn_patrol)_(nte)", 4, 20),
        ]),
        wiki: Some(json!({ "other_names": ["真红", "Shinku"], "body": "夜之 patrol 的队长。" })),
        related: Some(json!({
            "related_tags": [
                { "tag": tag("pink_hair", 0, 5000), "frequency": 0.42 },
                { "tag": tag("purple_horns", 0, 800), "frequency": 0.31 }
            ]
        })),
        ..Fake::default()
    };
    let (base, seen) = fake_danbooru(fake).await;

    let outcome = client(&base).lookup("Shinku (NTE)", 12, 20).await.unwrap();

    // 正文：角色 tag / 别名 / 换装版本 / 同现 tag 一个都不能少
    assert!(
        outcome.text.contains("归一成 `shinku_(nte)`"),
        "{}",
        outcome.text
    );
    assert!(
        outcome
            .text
            .contains("★ 角色 tag：`shinku_(nte)`（100 张）"),
        "{}",
        outcome.text
    );
    assert!(
        outcome
            .text
            .contains("别名（danbooru 记的 other names）：真红、Shinku"),
        "{}",
        outcome.text
    );
    assert!(
        outcome.text.contains("换装 / 其他版本 tag"),
        "{}",
        outcome.text
    );
    assert!(
        outcome.text.contains("shinku_(dawn_patrol)_(nte)"),
        "{}",
        outcome.text
    );
    assert!(
        outcome.text.contains("pink_hair purple_horns"),
        "同现 tag 要按频率顺序列出：{}",
        outcome.text
    );

    // meta 的键**精确**（少一个键 = 角色卡少一块内容）
    let mut keys: Vec<String> = outcome.meta.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "character",
            "exact",
            "normalizedTag",
            "provider",
            "query",
            "verifiedAt",
            "wikiUrl"
        ]
    );
    assert_eq!(outcome.meta["character"]["tag"], "shinku_(nte)");
    assert_eq!(outcome.meta["character"]["postCount"], 100);
    assert_eq!(outcome.meta["character"]["deprecated"], false);
    assert_eq!(
        outcome.meta["character"]["variants"],
        json!(["shinku_(dawn_patrol)_(nte)"])
    );
    assert_eq!(
        outcome.meta["character"]["otherNames"],
        json!(["真红", "Shinku"])
    );
    assert_eq!(outcome.meta["character"]["wikiMissing"], false);
    assert_eq!(
        outcome.meta["character"]["relatedTags"][1]["name"],
        "purple_horns"
    );
    assert_eq!(
        outcome.meta["character"]["relatedTags"][1]["frequency"],
        0.31
    );
    assert_eq!(outcome.meta["exact"][0]["category"], "character");
    assert!(
        outcome.meta["verifiedAt"].as_str().unwrap().ends_with('Z'),
        "核对时间要可追溯"
    );

    // 请求侧：模糊那一路**必须带通配符**，UA 必须是短的那一种
    let requests = seen.lock().unwrap().clone();
    let tags: Vec<&Seen> = requests.iter().filter(|r| r.path == "/tags.json").collect();
    assert_eq!(tags.len(), 2, "精确 + 模糊两趟都要发");
    assert!(
        tags.iter()
            .any(|r| r.query.contains("name_matches%5D=*shinku_%28nte%29*")),
        "模糊搜索没有带通配符（编码成 %2A 就搜不到换装版本了）：{tags:?}"
    );
    assert!(
        tags.iter().all(|r| r.user_agent == USER_AGENT),
        "UA 必须是短的那一种（长的会被 Cloudflare 拦）：{tags:?}"
    );
    assert!(
        requests
            .iter()
            .any(|r| r.path == "/related_tag.json" && r.query.contains("category=General")),
        "同现 tag 要限定 General 类：{requests:?}"
    );
}

#[tokio::test]
async fn wiki_404_是正常情况不许把已有结果一起丢掉() {
    let fake = Fake {
        exact: json!([tag("shinku_(nte)", 4, 100)]),
        fuzzy: json!([tag("shinku_(nte)", 4, 100)]),
        wiki: None, // → 404
        related: Some(
            json!({ "related_tags": [{ "tag": tag("pink_hair", 0, 5), "frequency": 0.5 }] }),
        ),
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;

    let outcome = client(&base).lookup("shinku", 12, 20).await.unwrap();
    assert!(
        outcome.text.contains("★ 角色 tag：`shinku_(nte)`"),
        "角色 tag 不该因为 wiki 404 丢掉：{}",
        outcome.text
    );
    assert!(
        outcome.text.contains("这个 tag 没有 wiki 页（正常情况"),
        "{}",
        outcome.text
    );
    assert_eq!(outcome.meta["character"]["wikiMissing"], true);
    assert!(
        outcome.meta["character"].get("wiki").is_none() || outcome.meta["character"]["wiki"] == "",
        "{}",
        outcome.meta
    );
    // 同现 tag 照旧拿到了（它就是"规范 tag 栈"的来源）
    assert!(outcome.text.contains("pink_hair"), "{}", outcome.text);
}

#[tokio::test]
async fn 同现tag_接口挂了只少一条增值信息不拖垮主结果() {
    let fake = Fake {
        exact: json!([tag("shinku_(nte)", 4, 100)]),
        fuzzy: json!([tag("shinku_(nte)", 4, 100)]),
        wiki: Some(json!({ "other_names": [], "body": "" })),
        related: None, // → 404
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;
    let outcome = client(&base).lookup("shinku", 12, 20).await.unwrap();
    assert!(
        outcome.text.contains("同现 tag：没拿到"),
        "{}",
        outcome.text
    );
    assert!(outcome.text.contains("★ 角色 tag"), "{}", outcome.text);

    // 但 429 / 5xx **仍然要报错** —— 那是"没查成"，与"没有"是两回事
    let fake = Fake {
        exact: json!([tag("shinku_(nte)", 4, 100)]),
        fuzzy: json!([tag("shinku_(nte)", 4, 100)]),
        wiki: Some(json!({ "other_names": [] })),
        related: None,
        related_status: 429,
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;
    let err = client(&base).lookup("shinku", 12, 20).await.unwrap_err();
    assert_eq!(err.code, "DANBOORU_RATE_LIMIT");
}

#[tokio::test]
async fn cloudflare_挑战页必须单列不许说成没有这个角色() {
    let fake = Fake {
        tags_status: 403,
        tags_body: "<html><head><title>Just a moment...</title></head><body>Enable JavaScript and cookies to continue</body></html>".into(),
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;
    let err = client(&base).lookup("shinku", 12, 20).await.unwrap_err();
    assert_eq!(err.code, "DANBOORU_CLOUDFLARE", "{}", err.message);
    assert!(err.message.contains("Cloudflare"), "{}", err.message);
    assert!(
        err.message.contains("不要"),
        "要明确禁止说成没有这个角色：{}",
        err.message
    );
    assert!(
        err.message.contains("COMFYHUB_DANBOORU_UA"),
        "要给出换 UA 的那个口子：{}",
        err.message
    );
}

#[tokio::test]
async fn 限流与_5xx_是没查成_404_是没有这个tag() {
    for (status, expected) in [(429_u16, "DANBOORU_RATE_LIMIT"), (503, "DANBOORU_FAILED")] {
        let fake = Fake {
            tags_status: status,
            tags_body: "upstream down".into(),
            ..Fake::default()
        };
        let (base, _) = fake_danbooru(fake).await;
        let err = client(&base).lookup("shinku", 12, 20).await.unwrap_err();
        assert_eq!(err.code, expected, "status={status} → {}", err.message);
    }

    // 404（tag 不存在）与"服务不可用"必须分开
    let fake = Fake {
        tags_status: 404,
        tags_body: "no such tag".into(),
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;
    let err = client(&base)
        .lookup("不存在的东西", 12, 20)
        .await
        .unwrap_err();
    assert_eq!(err.code, "DANBOORU_NOT_FOUND");
    assert!(err.message.contains("别照这个写法出图"), "{}", err.message);
}

#[tokio::test]
async fn 四零三_不是挑战页时报_ua_被拒() {
    let fake = Fake {
        tags_status: 403,
        tags_body: "Forbidden: bad robot user agent".into(),
        ..Fake::default()
    };
    let (base, _) = fake_danbooru(fake).await;
    let err = client(&base).lookup("shinku", 12, 20).await.unwrap_err();
    assert_eq!(err.code, "DANBOORU_UA_REJECTED", "{}", err.message);
    assert!(
        err.message.contains(USER_AGENT),
        "要把当前用的 UA 说出来：{}",
        err.message
    );
}

#[tokio::test]
async fn 没有解析出角色tag时要说下一步() {
    // 只有 general tag，没有 category=4 → 正文要给出"下一步怎么办"，而不是死路
    let fake = Fake {
        exact: json!([tag("shinku", 0, 900)]),
        fuzzy: json!([tag("shinku", 0, 900)]),
        ..Fake::default()
    };
    let (base, seen) = fake_danbooru(fake).await;
    let outcome = client(&base).lookup("shinku", 12, 20).await.unwrap();
    assert!(
        outcome.text.contains("没有解析出角色 tag"),
        "{}",
        outcome.text
    );
    assert!(
        outcome.text.contains("问用户这个角色出自哪部作品"),
        "{}",
        outcome.text
    );
    assert!(
        outcome.meta.get("character").is_none(),
        "没解析出角色就不该有 character 键：{}",
        outcome.meta
    );
    assert!(
        outcome.meta["candidates"].as_array().unwrap().len() == 1,
        "{}",
        outcome.meta
    );
    // 没有角色 tag 时**不发** wiki / related（一次查询最多 4 个请求，别乱发）
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        !paths.iter().any(|p| p.starts_with("/wiki_pages")),
        "{paths:?}"
    );
    assert!(!paths.iter().any(|p| p == "/related_tag.json"), "{paths:?}");
}
