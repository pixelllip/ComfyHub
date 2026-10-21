//! ComfyUI `/object_info` 的**进程内缓存**。
//!
//! # 为什么必须有
//!
//! `/object_info` 是**兆字节级**的响应：真机上（装了一批自定义节点包）实测 5~15MB。
//! 以前每打开一次蓝图、每学一次配方都**重新拉一遍 + 重新解析一遍**，于是
//! "点开蓝图"要等一两秒甚至更久，而 ComfyUI 自己打开一份工作流是瞬间的 ——
//! 用户 2026-10-18 的原话："蓝图加载速度过慢，和 comfy 不是一个量级"。
//!
//! # 三条纪律
//!
//! 1. **只缓存成功**。拿不到就回 `None`，下次照样重试 —— ComfyUI 刚起来 / 刚重启时
//!    不该被一个负缓存挡住（"我明明开了 ComfyUI 它说没连上"是最难查的那种）。
//! 2. **key 是地址**。用户换了实例（改配置里的 ComfyUI 地址）天然失效，不用手工清。
//! 3. **存 `Arc`**。`serde_json::Map` 的深拷贝是逐节点重新分配，10MB 的树拷一次
//!    要几百毫秒 —— 那会把缓存省下来的时间又原样还回去。取用一律只加引用计数。
//!
//! 有效期见 [`TTL`]：短到"用户装了新节点包，一分钟内就能看到"，
//! 长到"连着开几次蓝图只走一次网络"。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// 缓存有效期。
///
/// 60 秒太短：用户点开一份工作流、改个参数、再点开另一份，中间很容易过一分钟，
/// 于是每隔一会儿就有一次"冷"的兆字节请求（真机 4.5MB / ~550ms），体感就是"蓝图时不时卡一下"。
/// `/object_info` 只在装 / 卸节点包时变，10 分钟足够跟上，又不会让"刚装的节点包"永远看不到
/// （重启后端 / 换地址都会重新取）。
pub const TTL: Duration = Duration::from_secs(600);

/// 一次成功的取数。
struct Entry {
    /// 取数用的 ComfyUI 基地址（不带尾斜杠）。
    url: String,
    at: Instant,
    value: Arc<Map<String, Value>>,
}

/// 见模块头。默认就是"空缓存"，不需要参数。
#[derive(Default)]
pub struct ObjectInfoCache {
    entry: Mutex<Option<Entry>>,
}

impl ObjectInfoCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 命中就回一份 `Arc`（只加引用计数，不拷数据）。
    pub fn get(&self, url: &str) -> Option<Arc<Map<String, Value>>> {
        self.get_at(url, Instant::now())
    }

    /// 同 [`Self::get`]，但"现在"由调用方给 —— 用例据此把时间推进去，不用真等 60 秒。
    pub fn get_at(&self, url: &str, now: Instant) -> Option<Arc<Map<String, Value>>> {
        let guard = self.entry.lock().ok()?;
        let entry = guard.as_ref()?;
        if entry.url != url {
            return None;
        }
        if now.saturating_duration_since(entry.at) >= TTL {
            return None;
        }
        Some(Arc::clone(&entry.value))
    }

    pub fn put(&self, url: &str, value: Arc<Map<String, Value>>) {
        self.put_at(url, value, Instant::now());
    }

    pub fn put_at(&self, url: &str, value: Arc<Map<String, Value>>, now: Instant) {
        if let Ok(mut guard) = self.entry.lock() {
            *guard = Some(Entry { url: url.to_string(), at: now, value });
        }
    }

    /// 清空（改了 ComfyUI 地址 / 用户手动"重连"时用）。
    pub fn clear(&self) {
        if let Ok(mut guard) = self.entry.lock() {
            *guard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info(classes: &[&str]) -> Arc<Map<String, Value>> {
        let mut map = Map::new();
        for class in classes {
            map.insert((*class).to_string(), json!({ "input": {} }));
        }
        Arc::new(map)
    }

    #[test]
    fn 命中之后拿到的是同一份数据_而且是引用共享() {
        let cache = ObjectInfoCache::new();
        let value = info(&["KSampler", "CheckpointLoaderSimple"]);
        cache.put("http://127.0.0.1:8188", Arc::clone(&value));

        let hit = cache.get("http://127.0.0.1:8188").expect("刚放进去就该命中");
        assert_eq!(hit.len(), 2);
        assert!(hit.contains_key("KSampler"));
        // Arc 共享：同一个分配，**没有深拷贝**
        assert!(Arc::ptr_eq(&hit, &value), "缓存必须只加引用计数，不许拷贝整棵树");
    }

    #[test]
    fn 地址变了就不命中_不用手工清缓存() {
        let cache = ObjectInfoCache::new();
        cache.put("http://127.0.0.1:8188", info(&["KSampler"]));
        assert!(cache.get("http://127.0.0.1:8188").is_some());
        assert!(
            cache.get("http://127.0.0.1:8189").is_none(),
            "换了实例必须当没缓存 —— 否则会拿上一个实例的节点定义去判新实例"
        );
    }

    #[test]
    fn 过期之后不命中_但过期前一刻还命中() {
        let cache = ObjectInfoCache::new();
        let start = Instant::now();
        cache.put_at("http://x", info(&["A"]), start);

        assert!(cache.get_at("http://x", start + TTL - Duration::from_millis(1)).is_some());
        assert!(
            cache.get_at("http://x", start + TTL).is_none(),
            "到点即失效：边界取 >=，免得 0 秒的缓存被当永久"
        );
    }

    #[test]
    fn 空缓存不命中_也不会崩() {
        let cache = ObjectInfoCache::new();
        assert!(cache.get("http://x").is_none());
        cache.clear();
        assert!(cache.get("http://x").is_none());
    }

    #[test]
    fn clear_之后立刻不命中() {
        let cache = ObjectInfoCache::new();
        cache.put("http://x", info(&["A"]));
        cache.clear();
        assert!(cache.get("http://x").is_none());
    }
}
