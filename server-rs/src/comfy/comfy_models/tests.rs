//! `comfy_models` 的单元测试（主体代码在 [`comfy_models`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn object(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    m
}

/// `/object_info` 里一个 loader 节点的形状（枚举 + tooltip）。
fn loader(cls: &str, input: &str, names: &[&str]) -> (String, Value) {
    (
        cls.to_string(),
        json!({
            "input": { "required": { input: [names, {"tooltip": "x"}] } },
            "output": [],
        }),
    )
}

#[test]
fn 分词把驼峰与数字边界都切开() {
    assert_eq!(
        ModelName::tokens("krea2TurboOfficialComfy_krea2TurboInt8.safetensors"),
        vec!["krea", "2", "turbo", "official", "comfy", "int", "8"]
    );
    // 去扩展名：`flux1dev.safetensors` → flux / 1 / dev
    assert_eq!(
        ModelName::tokens("flux1dev.safetensors"),
        vec!["flux", "1", "dev"]
    );
    // 没有扩展名时整串都算
    assert_eq!(ModelName::tokens("anima-base"), vec!["anima", "base"]);
}

/// 三个实测例子（Kotlin 文档里那张表）。
#[test]
fn 相似度用的是词根而不是字面() {
    let want = "krea2TurboOfficialComfy_krea2TurboInt8.safetensors";
    assert!(ModelName::similarity(want, "krea2_turbo_int8.safetensors") >= MIN_SCORE);
    assert!(ModelName::similarity(want, want) == 1.0);
    // 字面很像但确实是两份模型 —— 高过阈值是**有意**的（工具不替模型做决定）
    let s = ModelName::similarity(
        "sd_xl_base_1.0.safetensors",
        "sd_xl_refiner_1.0.safetensors",
    );
    assert!((MIN_SCORE..1.0).contains(&s), "{s}");
    assert!(ModelName::same(
        "Krea2_Turbo.safetensors",
        "krea2 turbo.safetensors"
    ));
    assert!(!ModelName::same(
        "sd_xl_base_1.0.safetensors",
        "sd_xl_refiner_1.0.safetensors"
    ));
}

#[test]
fn 查询词先归一化去重再截断并粘回数字() {
    // ⚠ 是**小写**：`tokens()` 里已经 `to_lowercase()` 过了（Kotlin 的 KDoc 例子里写成
    // `krea2 Turbo Int8`，但实现给的是小写 —— 照实现抄，不照注释抄）
    assert_eq!(
        ModelName::lookup_query("krea2TurboOfficialComfy_krea2TurboInt8.safetensors"),
        "krea2 turbo int8"
    );
    // 停用词被吃光之后只剩那个**量化位宽**的数字（int8 的 `8` 是最有信息量的一位）
    assert_eq!(ModelName::lookup_query("model_final_v1.safetensors"), "1");
    // 全被停用词吃掉时退回原文件名（空查询词会被 civitai_search 当场拒掉）
    assert_eq!(
        ModelName::lookup_query("model_final_v.safetensors"),
        "model_final_v.safetensors"
    );
}

#[test]
fn 从_object_info_抠出清单并去重() {
    let mut m = Map::new();
    for (k, v) in [
        loader(
            "CheckpointLoaderSimple",
            "ckpt_name",
            &["a.safetensors", "sub\\b.safetensors"],
        ),
        loader("LoraLoader", "lora_name", &["c.safetensors"]),
    ] {
        m.insert(k, v);
    }
    let inv = from_object_info(Some(&m)).unwrap();
    assert_eq!(inv.source, Source::ObjectInfo);
    assert_eq!(inv.models.len(), 3);
    assert!(inv.models.iter().all(|x| x.visible_to_instance));
    // 一条枚举都没有的 `/object_info` → None（退回扫盘，空清单比"没清单"更危险）
    assert!(from_object_info(Some(&object(&[("Foo", json!({}))]))).is_none());
}

#[test]
fn 扫盘认后缀且深度与体积如实() {
    let dir = crate::infra::testpaths::scratch("models-scan");
    let loras = dir.join("loras");
    std::fs::create_dir_all(loras.join("nested")).unwrap();
    std::fs::write(loras.join("a.safetensors"), b"1234").unwrap();
    std::fs::write(loras.join("nested").join("b.ckpt"), b"1").unwrap();
    std::fs::write(loras.join("readme.txt"), b"x").unwrap();
    // ComfyUI / Civitai 的 sidecar：**不是模型**，收进来会造出假的"实例看不到"警报
    std::fs::write(loras.join("a.metadata.json"), b"{}").unwrap();
    std::fs::write(loras.join("A.METADATA.JSON"), b"{}").unwrap();

    let inv = scan(None, std::slice::from_ref(&dir));
    assert_eq!(inv.source, Source::Scan);
    let names: Vec<String> = inv.models.iter().map(|m| m.name.clone()).collect();
    assert!(names.contains(&"a.safetensors".to_string()), "{names:?}");
    // 子目录里的用 `/` 分隔（与 /object_info 给的形状一致）
    assert!(names.contains(&"nested/b.ckpt".to_string()), "{names:?}");
    // 非模型后缀一律不收
    assert!(!names.iter().any(|n| n.ends_with(".txt")));
    // ⚠ sidecar 一律不收（大小写都算）——真实机上这 3 个曾把报告变成假警报
    assert!(
        !names
            .iter()
            .any(|n| n.to_lowercase().ends_with(".metadata.json")),
        "{names:?}"
    );
    let a = inv
        .models
        .iter()
        .find(|m| m.name == "a.safetensors")
        .unwrap();
    assert_eq!(a.bytes, Some(4));
    assert!(a.path.is_some());
    // 扫盘来源**不能**声称"实例看得到"
    assert!(!a.visible_to_instance);
    std::fs::remove_dir_all(&dir).ok();
}

/// 并集才是那个"盘上有、实例看不到"的判据（2026-09-22 事故的解药）。
#[test]
fn 并集把实例看不到的那些单列出来() {
    let dir = crate::infra::testpaths::scratch("models-merge");
    let ckpts = dir.join("checkpoints");
    std::fs::create_dir_all(&ckpts).unwrap();
    std::fs::write(ckpts.join("on_disk_only.safetensors"), b"12").unwrap();
    std::fs::write(ckpts.join("seen.safetensors"), b"1234").unwrap();

    let mut oi = Map::new();
    let (k, v) = loader("CheckpointLoaderSimple", "ckpt_name", &["seen.safetensors"]);
    oi.insert(k, v);

    let inv = inventory(Some(&oi), None, std::slice::from_ref(&dir));
    assert!(inv.merged);
    assert_eq!(inv.source, Source::ObjectInfo, "权威来源仍然是实例");
    assert_eq!(inv.models.len(), 2);
    assert_eq!(inv.disk_only.len(), 1);
    assert_eq!(inv.disk_only[0].name, "on_disk_only.safetensors");
    // 两边都有的那一条：能加载 + 带上盘上的体积
    let seen = inv
        .models
        .iter()
        .find(|m| m.name == "seen.safetensors")
        .unwrap();
    assert!(seen.visible_to_instance);
    assert_eq!(seen.bytes, Some(4));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn 候选先本类再跨目录且按分数排() {
    let inv = Inventory {
        source: Source::Scan,
        models: vec![
            LocalModel {
                name: "krea2_turbo_int8.safetensors".into(),
                folder: "checkpoints".into(),
                path: None,
                bytes: None,
                visible_to_instance: true,
            },
            LocalModel {
                name: "sd15.safetensors".into(),
                folder: "loras".into(),
                path: None,
                bytes: None,
                visible_to_instance: true,
            },
        ],
        models_dir: None,
        truncated: false,
        disk_only: Vec::new(),
        merged: false,
    };
    let hits = inv.candidates_for(
        "checkpoints",
        "krea2TurboOfficialComfy_krea2TurboInt8.safetensors",
    );
    assert_eq!(hits.len(), 1);
    assert!(hits[0].same_folder);
    // 分数不达标的（sd15）一条都不给 —— **不编**
    assert!(hits.iter().all(|h| h.model.name != "sd15.safetensors"));
    // 沾边的那些只在 near_misses 里出现，且要有长度 ≥3 的公共词
    assert!(inv
        .near_misses_for("krea2TurboInt8.safetensors", 3)
        .is_empty());
}

#[test]
fn 图里点名的模型只认字符串值不认连线() {
    let graph = object(&[
        (
            "1",
            json!({"class_type": "CheckpointLoaderSimple",
                   "inputs": {"ckpt_name": "a.safetensors"},
                   "_meta": {"title": "底模"}}),
        ),
        // 值是数组（连线）→ 跳过（那是"从别的节点来的"）
        (
            "2",
            json!({"class_type": "VAELoader", "inputs": {"vae_name": ["3", 0]}}),
        ),
        // 认不出来的类 → 不猜
        (
            "3",
            json!({"class_type": "MyThirdPartyThing", "inputs": {"model": "x.safetensors"}}),
        ),
        // 空串不算"要装一个模型"
        (
            "4",
            json!({"class_type": "LoraLoader", "inputs": {"lora_name": "  "}}),
        ),
    ]);
    let reqs = requirements(&graph, None);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].override_key(), "1.ckpt_name");
    assert_eq!(reqs[0].folder, "checkpoints");
    assert_eq!(reqs[0].title.as_deref(), Some("底模"));
}

#[test]
fn 从_object_info_认出第三方加载器的模型输入() {
    let mut m = Map::new();
    let (k, v) = loader("MyThirdPartyLoader", "model_name", &["x.safetensors"]);
    m.insert(k, v);
    // 自由文本输入（第一项是类型名 `STRING`）不能被误认
    m.insert(
        "FreeText".to_string(),
        json!({"input": {"required": {"text": ["STRING", {"multiline": true}]}}}),
    );
    let found = enumerate_model_inputs(Some(&m));
    assert!(found
        .get("MyThirdPartyLoader")
        .unwrap()
        .contains("model_name"));
    assert!(!found.contains_key("FreeText"));
}
