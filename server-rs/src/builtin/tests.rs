//! `builtin` 的单元测试（主体代码在 [`builtin`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn builtin(id: &str, modalities: &[&str], tools: bool, reasoning: bool) -> BuiltinModel {
    BuiltinModel {
        id: id.to_string(),
        display_name: id.to_uppercase(),
        context_window: Some(1000),
        input_modalities: modalities.iter().map(|s| s.to_string()).collect(),
        tools,
        parallel_tools: false,
        reasoning,
        thinking_format: None,
        thinking_efforts: Default::default(),
    }
}

fn row_of(model: &BuiltinModel, provider_id: &str) -> AiModelDto {
    to_model_dto(model, provider_id, None)
}

/// 冻结副本本身要能读、版本号要对得上、每条都有 id（它是**出厂默认值**的唯一来源）。
#[test]
fn 冻结副本可读而且不为空() {
    let parsed = catalog().expect("内置目录必须能解析");
    assert!(!parsed.providers.is_empty());
    assert!(!parsed.version.trim().is_empty());
    for provider in &parsed.providers {
        assert!(!provider.models.is_empty(), "{} 一个模型都没有", provider.id);
        for model in &provider.models {
            assert!(!model.id.trim().is_empty());
            assert!(!model.display_name.trim().is_empty());
        }
    }
    // provider 行也要能过我们那套校验（协议 / URL / 信任档 / 凭据引用名）
    for provider in &parsed.providers {
        to_provider_dto(provider).expect("内置 provider 必须过校验");
    }
    // 每条模型也要能过模型校验
    for provider in &parsed.providers {
        for model in &provider.models {
            crate::ai::validate_model(&to_model_dto(model, &provider.id, None))
                .unwrap_or_else(|e| panic!("{} 过不了模型校验：{}", model.id, e.human()));
        }
    }
}

/// `planSeed` 的三条分支 + "库里多出来的不算任何一类"（永远不删）。
#[test]
fn 对照结论把缺的_一致的_不一致的分开() {
    let catalog = vec![
        builtin("a", &["text"], true, false),
        builtin("b", &["text", "image"], true, false),
        builtin("c", &["text"], false, true),
    ];
    let mut existing = std::collections::BTreeMap::new();
    existing.insert("b".to_string(), row_of(&catalog[1], "p"));
    // c 的能力与目录不同（库里说它不支持推理）
    let mut c_row = row_of(&catalog[2], "p");
    c_row.reasoning = false;
    existing.insert("c".to_string(), c_row);
    // 用户自己加的：不属于任何一类
    existing.insert("user-only".to_string(), row_of(&builtin("user-only", &["text"], true, false), "p"));

    let plan = plan_seed(&catalog, &existing);
    assert_eq!(plan.added, vec!["a"]);
    assert_eq!(plan.kept, vec!["b"]);
    assert_eq!(plan.divergent, vec!["c"]);
    assert!(!plan.added.contains(&"user-only".to_string()), "用户自己加的不进任何一类");
    assert!(!plan.divergent.contains(&"user-only".to_string()));
}

/// 能力不同的判据：**只比能力那八样**，显示名 / 启用状态 / 附件上限都不算。
#[test]
fn 只比能力不比用户的地盘() {
    let model = builtin("m", &["text", "image"], true, true);
    let base = row_of(&model, "p");
    assert!(!capability_differs(&model, &base), "同一条不算不同");

    // 用户改过的：显示名 / 启用 / 附件上限 —— 都不该判成"能力不同"
    let mut renamed = base.clone();
    renamed.display_name = "用户改的名字".into();
    renamed.enabled = false;
    renamed.max_attachment_bytes = Some(1024);
    assert!(!capability_differs(&model, &renamed));

    // 模态顺序不同不算不同（比的是排序后的集合）
    let mut reordered = base.clone();
    reordered.input_modalities = vec!["image".into(), "text".into()];
    assert!(!capability_differs(&model, &reordered));

    // 真正的能力不同：少一个模态 / 推理关了 / 上下文变了 / 来源变了
    for mutate in [
        (|m: &mut AiModelDto| m.input_modalities = vec!["text".into()]) as fn(&mut AiModelDto),
        |m: &mut AiModelDto| m.reasoning = false,
        |m: &mut AiModelDto| m.context_window = Some(7),
        |m: &mut AiModelDto| m.capability_source = "manual".into(),
    ] {
        let mut changed = base.clone();
        mutate(&mut changed);
        assert!(capability_differs(&model, &changed), "{changed:?} 应当算不同");
    }
}

/// 模式 → 动作：ADD_MISSING 只插入、REFRESH 只刷新。
#[test]
fn 两种模式的动作互不重叠() {
    let plan = SeedPlan { added: vec!["a".into()], kept: vec!["b".into()], divergent: vec!["c".into()] };
    let (insert, refresh) = decide_actions(SeedMode::AddMissing, &plan);
    assert_eq!(insert, vec!["a"]);
    assert!(refresh.is_empty(), "补齐模式**绝不对齐**（会冲掉用户改过的能力）");
    let (insert, refresh) = decide_actions(SeedMode::RefreshCapabilities, &plan);
    assert!(insert.is_empty(), "对齐模式**绝不新增**");
    assert_eq!(refresh, vec!["c"]);
}

/// `?mode=` 的取值：认可的那几个，其它一律报错（不静默按默认走）。
#[test]
fn 同步模式的取值与_kotlin_一致() {
    assert_eq!(SeedMode::parse(None).unwrap(), SeedMode::AddMissing);
    assert_eq!(SeedMode::parse(Some("")).unwrap(), SeedMode::AddMissing);
    assert_eq!(SeedMode::parse(Some("add-missing")).unwrap(), SeedMode::AddMissing);
    assert_eq!(SeedMode::parse(Some("refresh")).unwrap(), SeedMode::RefreshCapabilities);
    assert_eq!(SeedMode::parse(Some("REFRESH-CAPABILITIES")).unwrap(), SeedMode::RefreshCapabilities);
    let err = SeedMode::parse(Some("whatever")).unwrap_err();
    assert_eq!(
        err.human(),
        "未知的同步模式：whatever（可选 add-missing / refresh-capabilities）"
    );
}

/// 目录条目 → 写库形状：附件列**故意留空**（写了就是假承诺），能力来源是 builtin。
#[test]
fn 写进库的形状不带假的附件能力() {
    let model = builtin("m", &["text", "image"], true, false);
    let dto = to_model_dto(&model, "p", None);
    assert!(dto.attachment_transports.is_empty(), "适配器还没实现附件传输");
    assert!(dto.mime_allowlist.is_empty());
    assert!(dto.max_attachment_bytes.is_none());
    assert_eq!(dto.capability_source, "builtin");
    assert!(dto.enabled);
}
