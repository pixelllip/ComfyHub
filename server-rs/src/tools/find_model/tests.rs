//! `find_model` 的单元测试（主体代码在 [`find_model`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
// -----------------------------------------------------------------------
//  纯函数
// -----------------------------------------------------------------------

/// 三条线索的可信度顺序：**文件名 → 路径/仓库名 → pipeline_tag**。
#[test]
fn 类型推断的三条线索按可信度排() {
    assert_eq!(
        ModelKind::from_civitai(Some("Checkpoint")),
        ModelKind::Checkpoint
    );
    assert_eq!(ModelKind::from_civitai(Some("LORA")), ModelKind::Lora);
    assert_eq!(ModelKind::from_civitai(Some("LoCon")), ModelKind::Lora);
    assert_eq!(ModelKind::from_civitai(Some("DoRA")), ModelKind::Lora);
    assert_eq!(
        ModelKind::from_civitai(Some("TextualInversion")),
        ModelKind::Embedding
    );
    assert_eq!(
        ModelKind::from_civitai(Some("Upscaler")),
        ModelKind::Upscaler
    );
    assert_eq!(ModelKind::from_civitai(Some("VAE")), ModelKind::Vae);
    assert_eq!(
        ModelKind::from_civitai(Some("Controlnet")),
        ModelKind::Controlnet
    );
    assert_eq!(ModelKind::from_civitai(Some("别的")), ModelKind::Unknown);
    assert_eq!(ModelKind::from_civitai(None), ModelKind::Unknown);

    assert_eq!(
        ModelKind::from_task(Some("text-to-image")),
        ModelKind::Diffusion
    );
    assert_eq!(
        ModelKind::from_task(Some("diffusers")),
        ModelKind::Diffusion
    );
    assert_eq!(ModelKind::from_task(Some("peft")), ModelKind::Lora);
    assert_eq!(ModelKind::from_task(Some("whatever")), ModelKind::Unknown);

    // 路径线索：整段目录名，或夹在名字里的 `_loras` / `lora-`
    assert_eq!(
        ModelKind::from_path_hint(Some("split_files/diffusion_models/a.safetensors")),
        Some(ModelKind::Diffusion)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("Omnico/Krea2_turbo_diff_loras/Moody5.safetensors")),
        Some(ModelKind::Lora)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("a/lora-x/b.safetensors")),
        Some(ModelKind::Lora)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/vae/ae.safetensors")),
        Some(ModelKind::Vae)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/checkpoints/x.safetensors")),
        Some(ModelKind::Checkpoint)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/text_encoders/t5.safetensors")),
        Some(ModelKind::TextEncoder)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/clip_vision/c.safetensors")),
        Some(ModelKind::ClipVision)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/upscale_models/u.pth")),
        Some(ModelKind::Upscaler)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/embeddings/e.pt")),
        Some(ModelKind::Embedding)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/controlnet/c.safetensors")),
        Some(ModelKind::Controlnet)
    );
    assert_eq!(
        ModelKind::from_path_hint(Some("w/misc/x.safetensors")),
        None
    );
    assert_eq!(ModelKind::from_path_hint(None), None);

    // 文件名线索（比仓库名更硬：`pytorch_lora_weights.safetensors`）
    assert_eq!(
        ModelKind::from_file_name(Some("sub/pytorch_lora_weights.safetensors")),
        Some(ModelKind::Lora)
    );
    assert_eq!(
        ModelKind::from_file_name(Some("control_net_x.pth")),
        Some(ModelKind::Controlnet)
    );
    assert_eq!(
        ModelKind::from_file_name(Some("ae.safetensors")),
        Some(ModelKind::Vae)
    );
    assert_eq!(
        ModelKind::from_file_name(Some("clip_vision_x.safetensors")),
        Some(ModelKind::ClipVision)
    );
    assert_eq!(
        ModelKind::from_file_name(Some("text_encoder_x.safetensors")),
        Some(ModelKind::TextEncoder)
    );
    assert_eq!(
        ModelKind::from_file_name(Some("some_embedding.pt")),
        Some(ModelKind::Embedding)
    );
    assert_eq!(ModelKind::from_file_name(Some("krea2.safetensors")), None);

    // 顺序：文件名赢过路径与 task（文件名是 `pytorch_lora_weights.safetensors`，
    // 路径却把它放在 `vae/` 下 —— 前者更硬）
    assert_eq!(
        ModelKind::infer(
            Some("repo/vae/pytorch_lora_weights.safetensors"),
            Some("repo/vae/x.safetensors"),
            Some("text-to-image")
        ),
        ModelKind::Lora
    );
    // 没文件名时路径赢过 task
    assert_eq!(
        ModelKind::infer(
            None,
            Some("repo/loras/x.safetensors"),
            Some("text-to-image")
        ),
        ModelKind::Lora
    );
    assert_eq!(
        ModelKind::infer(None, None, Some("text-to-image")),
        ModelKind::Diffusion
    );
    assert_eq!(ModelKind::infer(None, None, None), ModelKind::Unknown);

    // diffusers 格式的判据来自仓库自己的声明
    assert!(ModelKind::is_diffusers_format(Some("diffusers"), &[]));
    assert!(ModelKind::is_diffusers_format(
        None,
        &["DIFFUSERS".to_string()]
    ));
    assert!(!ModelKind::is_diffusers_format(
        Some("transformers"),
        &["safetensors".to_string()]
    ));
}

#[test]
fn install_into_与体积标签() {
    // 认不出类型时**不给目录**（宁可说"说不准"，也不猜一个）
    assert_eq!(ModelKind::Unknown.folder(), None);
    assert_eq!(ModelKind::Lora.folder(), Some("loras"));
    assert_eq!(ModelKind::Diffusion.folder(), Some("diffusion_models"));
    assert_eq!(ModelKind::TextEncoder.folder(), Some("text_encoders"));

    let mut candidate = candidate_of("x", ModelKind::Lora);
    assert_eq!(candidate.install_into(), Some("loras"));
    candidate.size_bytes = None;
    assert_eq!(candidate.size_label(), None, "拿不到体积就不编");
    candidate.size_bytes = Some(14_145_495 * 1024);
    assert_eq!(candidate.size_label().as_deref(), Some("13.49 GB"));
    // 几 MB 的 LoRA：用 `%.1f` 才不会是 "0 MB"（那看起来像空文件）
    candidate.size_bytes = Some(3 * 1024 * 1024);
    assert_eq!(candidate.size_label().as_deref(), Some("3.0 MB"));
}

#[test]
fn sha256_只能是_64_位十六进制() {
    assert!(is_sha256(Some(&"a".repeat(64))));
    assert!(is_sha256(Some(&"A".repeat(64))));
    assert!(!is_sha256(Some("ABCDEF1234")), "AutoV2 那种短码不是 sha256");
    assert!(!is_sha256(Some(&"g".repeat(64))));
    assert!(!is_sha256(None));
}

#[test]
fn 权重文件判据与扩展名表同源() {
    assert!(looks_like_weight("a/b/c.safetensors"));
    assert!(looks_like_weight("x.CKPT"), "扩展名大小写不敏感");
    assert!(looks_like_weight("m.gguf"));
    // `.yaml` / `.json` 是 ComfyUI 可加载的配置，但**不是**"需要手动下载的文件"
    assert!(!looks_like_weight("config.json"));
    assert!(!looks_like_weight("model.yaml"));
    assert!(!looks_like_weight("noext"));
}

/// 合并去重：**同名不同写法**要合成一条，并且保留信息更全的那条。
///
/// ⚠ 键的**文件那一半只做小写归一**（不做 `_`/`-` 归一）—— 这是 Kotlin 的行为，
/// 不要"一并现代化"：归一之后"两个源上不同拼写的同名文件"会被误合并为一条。
#[test]
fn 跨源同名合成一条并保留信息更全的() {
    let mut civitai = candidate_of("krea2 Turbo Int8", ModelKind::Checkpoint);
    civitai.file_name = Some("krea2_turbo_int8.safetensors".into());
    civitai.sha256 = Some(sha('c'));
    civitai.size_bytes = Some(1000);
    civitai.base_model = None;
    civitai.source = ModelSource::Civitai;
    civitai.sources = vec![ModelSource::Civitai];

    let mut hf = candidate_of("krea2-turbo-int8", ModelKind::Checkpoint);
    hf.file_name = Some("KREA2_TURBO_INT8.safetensors".into());
    hf.base_model = Some("SDXL 1.0".into());
    hf.trained_words = vec!["krea2".into()];
    hf.source = ModelSource::HuggingFace;
    hf.sources = vec![ModelSource::HuggingFace];

    let merged = merge(vec![civitai.clone(), hf.clone()]);
    assert_eq!(merged.len(), 1, "词根一致 + 归一化文件名一致 → 一条");
    let one = &merged[0];
    assert_eq!(
        one.sources,
        vec![ModelSource::Civitai, ModelSource::HuggingFace],
        "来源并集按查询顺序"
    );
    assert_eq!(
        one.sha256.as_deref(),
        Some(sha('c').as_str()),
        "有 sha256 的那条当主"
    );
    assert_eq!(one.size_bytes, Some(1000), "主来源缺的字段从另一条补");
    assert_eq!(
        one.base_model.as_deref(),
        Some("SDXL 1.0"),
        "主来源没给底模时从另一条补"
    );
    assert_eq!(one.trained_words, vec!["krea2", "turbo"], "触发词取并集");
    assert_eq!(one.source, ModelSource::Civitai, "主来源不变");

    // 文件名不同 → 不合并（两个源上确实是两份东西）
    let mut other = hf.clone();
    other.file_name = Some("krea2-refiner.safetensors".into());
    assert_eq!(merge(vec![civitai.clone(), other]).len(), 2);

    // 分数：sha256(4) + 体积(2) + 触发词(1) + 直链(1)
    assert!(score(&merged[0]) >= score(&hf));
    assert_eq!(score(&civitai), 4 + 2 + 1 + 1);
    let mut bare = civitai.clone();
    bare.sha256 = None;
    bare.size_bytes = None;
    bare.trained_words = Vec::new();
    bare.download_url = None;
    assert_eq!(score(&bare), 0);
}

/// `type` 写错时**当场列出可选值**，而不是给一份空结果。
#[test]
fn type_归一与报错文案() {
    assert_eq!(parse_model_kind(None).unwrap(), None);
    assert_eq!(parse_model_kind(Some("   ")).unwrap(), None);
    assert_eq!(
        parse_model_kind(Some("lora")).unwrap(),
        Some(ModelKind::Lora)
    );
    assert_eq!(
        parse_model_kind(Some(" 底模 ")).unwrap(),
        Some(ModelKind::Checkpoint)
    );
    assert_eq!(
        parse_model_kind(Some("clip")).unwrap(),
        Some(ModelKind::TextEncoder)
    );
    assert_eq!(
        parse_model_kind(Some("esrgan")).unwrap(),
        Some(ModelKind::Upscaler)
    );
    let err = parse_model_kind(Some("Loraa")).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("LORA"), "{}", err.message);
    assert!(err.message.contains("Loraa"), "{}", err.message);
    assert!(
        err.message.ends_with("不传这个参数也可以。"),
        "{}",
        err.message
    );
}

// -----------------------------------------------------------------------
//  渲染（给模型看的那一份）
// -----------------------------------------------------------------------

fn candidate_of(name: &str, kind: ModelKind) -> ModelCandidate {
    ModelCandidate {
        name: name.to_string(),
        kind,
        base_model: Some("Flux.1 D".into()),
        trained_words: vec!["krea2".into(), "turbo".into()],
        size_bytes: Some(14_145_495 * 1024),
        download_url: Some(format!("https://example.test/dl/{name}")),
        page_url: Some("https://example.test/models/9".into()),
        sha256: Some(sha('a')),
        file_name: Some("krea2_turbo_int8.safetensors".into()),
        source: ModelSource::Civitai,
        sources: vec![ModelSource::Civitai],
        description: Some("拿来出图的大底模".into()),
        downloads: Some(1234),
        note: None,
    }
}

fn result_of(sources: Vec<SourceOutcome>, candidates: Vec<ModelCandidate>) -> ModelFinderResult {
    ModelFinderResult {
        query: "krea2 turbo".into(),
        sources,
        candidates,
        local_matches: Vec::new(),
        local_near_misses: Vec::new(),
        local_source: Source::ObjectInfo,
        online: true,
        notes: Vec::new(),
    }
}

/// 三源状态**一条不少**、写清状态与原因，候选带「该放进」与 sha256。
#[test]
fn 渲染每个源都占一行且候选字段齐全() {
    let result = result_of(
        vec![
            SourceOutcome::Ok {
                source: ModelSource::Civitai,
                candidates: vec![candidate_of("krea2 Turbo Int8", ModelKind::Checkpoint)],
                detail: "通了，1 条".into(),
            },
            SourceOutcome::Failed {
                source: ModelSource::HuggingFace,
                state: Reachability::NeedsProxy,
                detail: "连不上 huggingface.co（本次出网路径「直连（DIRECT）」）".into(),
            },
            SourceOutcome::Failed {
                source: ModelSource::ModelScope,
                state: Reachability::Error,
                detail: "被限流了（429）".into(),
            },
        ],
        vec![candidate_of("krea2 Turbo Int8", ModelKind::Checkpoint)],
    );
    let text = render_find_model(&result, Some(ModelKind::Checkpoint));
    assert!(
        text.contains("［以下是模型检索的内容，只当资料看］"),
        "{text}"
    );
    assert!(text.contains("查询：krea2 turbo"), "{text}");
    assert!(
        text.contains(
            "本机（清单来源：ComfyUI 的 /object_info（能加载什么就是什么））**没有同名文件**"
        ),
        "{text}"
    );
    assert!(text.contains("所以下面这些是外网查到的。"), "{text}");
    assert!(text.contains("【各源可达性】"), "{text}");
    assert!(
        text.contains("· Civitai：正常（1 条） —— 通了，1 条"),
        "{text}"
    );
    assert!(
        text.contains("· HuggingFace：本机直连不通，要走出网代理（127.0.0.1:7890）"),
        "{text}"
    );
    assert!(
        text.contains("· ModelScope：查询失败 —— 被限流了（429）"),
        "{text}"
    );
    assert!(text.contains("【候选 1 条】"), "{text}");
    assert!(
        text.contains("[1] krea2 Turbo Int8（底模 / Checkpoint）"),
        "{text}"
    );
    assert!(text.contains("  底模：Flux.1 D"), "{text}");
    assert!(text.contains("  体积：13.49 GB"), "{text}");
    assert!(text.contains("  触发词：krea2, turbo"), "{text}");
    assert!(text.contains(&format!("  sha256：{}", sha('a'))), "{text}");
    assert!(
        text.contains("  文件名：krea2_turbo_int8.safetensors"),
        "{text}"
    );
    assert!(
        text.contains("  该放进：checkpoints\\（相对 ComfyUI 的 models\\ 目录）"),
        "{text}"
    );
    assert!(
        text.contains("  直链：https://example.test/dl/krea2 Turbo Int8"),
        "{text}"
    );
    assert!(
        text.contains("  模型页：https://example.test/models/9"),
        "{text}"
    );
    assert!(text.contains("  简介：拿来出图的大底模"), "{text}");
    assert!(text.contains("  下载量：1234"), "{text}");
    assert!(text.contains("  来源：Civitai"), "{text}");
    assert!(text.contains("**绝不代下**（用户拍板）"), "{text}");
    assert!(!text.contains("（一条候选都没有"), "{text}");
}

/// 本机命中：**先说这一条**（"别急着让他去下"），并把"实例看不到"那条诊断翻成动作。
#[test]
fn 渲染本机命中时不让人去下载() {
    let hit = LocalModel {
        name: "krea2_turbo_int8.safetensors".into(),
        folder: "checkpoints".into(),
        path: Some(r"D:\models\checkpoints\krea2_turbo_int8.safetensors".into()),
        bytes: Some(13_900_000_000),
        visible_to_instance: false,
    };
    let mut result = result_of(
        ModelSource::ALL.iter().map(|s| skipped(*s, true)).collect(),
        Vec::new(),
    );
    result.local_matches = vec![hit.clone()];
    let text = render_find_model(&result, None);
    assert!(text.contains("**本机已经有这个文件了**"), "{text}");
    assert!(
        text.contains("· checkpoints/krea2_turbo_int8.safetensors（12.95GB）"),
        "{text}"
    );
    assert!(text.contains("**当前跑的 ComfyUI 实例看不到它**"), "{text}");
    assert!(text.contains("路径：D:\\models\\checkpoints"), "{text}");
    assert!(text.contains("重启 ComfyUI / 修好模型根"), "{text}");
    assert!(text.contains("**不要去下载**"), "{text}");
    assert!(text.contains("没查：本机已有同名模型"), "{text}");

    // 本机可见时不该出现"实例看不到"那句，也不该给"重启"的动作
    let mut result = result_of(
        ModelSource::ALL.iter().map(|s| skipped(*s, true)).collect(),
        Vec::new(),
    );
    result.local_matches = vec![LocalModel {
        visible_to_instance: true,
        ..hit
    }];
    let text = render_find_model(&result, None);
    assert!(!text.contains("实例看不到"), "{text}");
    assert!(!text.contains("重启 ComfyUI"), "{text}");
}

/// "没查到"与"没查成"要分开说；上游没给 sha256 也要如实标出来。
#[test]
fn 渲染把没查到与没查成分开说() {
    let mut result = result_of(
        vec![
            SourceOutcome::Ok {
                source: ModelSource::Civitai,
                candidates: Vec::new(),
                detail: "通了，但这一问没有结果".into(),
            },
            SourceOutcome::Failed {
                source: ModelSource::HuggingFace,
                state: Reachability::Gated,
                detail: "被拒绝（403）：HF 这类 gated 仓库要用户**先在模型页面上同意条款**".into(),
            },
            SourceOutcome::Failed {
                source: ModelSource::ModelScope,
                state: Reachability::NeedsLogin,
                detail: "需要登录 / 令牌（401）".into(),
            },
        ],
        Vec::new(),
    );
    result.notes = vec!["本次没有一条带 sha256（上游没给）".into()];
    let text = render_find_model(&result, None);
    assert!(
        text.contains("（一条候选都没有。如实告诉用户没查到 + 试过什么词"),
        "{text}"
    );
    assert!(
        text.contains("注意：这次有源查不成（HuggingFace=gated、ModelScope=needs_login）"),
        "{text}"
    );
    assert!(
        text.contains("\"没查到\"里有一部分是\"没查成\"，两件事要分开说。"),
        "{text}"
    );
    assert!(text.contains("\n· 本次没有一条带 sha256"), "{text}");

    // 有候选：类型认不出 + 有 note → 先指向那条说明
    let mut unknown = candidate_of("y", ModelKind::Unknown);
    unknown.note = Some("diffusers 格式".into());
    unknown.sha256 = None;
    let text = render_candidate(1, &unknown, Some(ModelKind::Lora));
    assert!(
        text.contains("该放进：**这个文件不一定能直接放进 ComfyUI**，见下面那条说明。"),
        "{text}"
    );
    assert!(text.contains("⚠ diffusers 格式"), "{text}");
    assert!(
        text.contains("（这一条上游没给 sha256，下完没法校验）"),
        "{text}"
    );

    // 没有 note 时按"无法确定 + 所需类型"返回，并附带文件名线索
    unknown.note = None;
    let text = render_candidate(2, &unknown, Some(ModelKind::Lora));
    assert!(
        text.contains("该放进：**说不准**（这个源没说清它是什么类型）—— "),
        "{text}"
    );
    assert!(
        text.contains("你说这次要的是 LoRA / LyCORIS，那大概率是 loras\\"),
        "{text}"
    );
    assert!(
        text.contains("（文件名里有线索：krea2_turbo_int8.safetensors）"),
        "{text}"
    );

    // 未说明所需类型 → 由工作流的加载器判断
    let text = render_candidate(3, &unknown, None);
    assert!(
        text.contains("让用户看工作流里那个加载器要的是哪一类"),
        "{text}"
    );

    // 多来源时标出"在哪些源上都有"
    let mut both = candidate_of("z", ModelKind::Lora);
    both.sources = vec![ModelSource::Civitai, ModelSource::HuggingFace];
    assert!(render_candidate(4, &both, None).contains("  ← 在 Civitai、HuggingFace 上都有"));
}

/// 结构化结果：键取舍（可空键省掉）+ `downloadsModel=false` + `sizeGB` 两位小数。
#[test]
fn 结构化结果的键取舍() {
    let mut result = result_of(
        ModelSource::ALL.iter().map(|s| skipped(*s, true)).collect(),
        Vec::new(),
    );
    result.local_matches = vec![LocalModel {
        name: "krea2_turbo_int8.safetensors".into(),
        folder: "checkpoints".into(),
        path: Some(r"D:\models\a.safetensors".into()),
        bytes: Some(13_900_000_000),
        visible_to_instance: false,
    }];
    result.local_near_misses = vec!["krea2_refiner.safetensors".into()];
    result.online = false;
    let value = to_find_model_json(&result);
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "query",
            "online",
            "localInventorySource",
            "localHit",
            "localMatches",
            "localNearMisses",
            "sources",
            "count",
            "candidates",
            "downloadsModel",
        ]
    );
    assert_eq!(value["online"], false);
    assert_eq!(value["localInventorySource"], "object_info");
    assert_eq!(value["localHit"], true);
    assert_eq!(
        value["downloadsModel"], false,
        "「绝不代下」也要写进结构化结果"
    );
    assert_eq!(
        value["localMatches"][0]["name"],
        "krea2_turbo_int8.safetensors"
    );
    assert_eq!(value["localMatches"][0]["visibleToInstance"], false);
    assert_eq!(
        value["localMatches"][0]["sizeGB"], 12.95,
        "round(gb*100)/100"
    );
    assert_eq!(value["sources"][0]["source"], "civitai");
    assert_eq!(value["sources"][0]["state"], "skipped");
    assert_eq!(value["sources"][0]["count"], 0);
    assert!(value["candidates"].as_array().unwrap().is_empty());
    assert!(value.get("notes").is_none(), "空 notes 不写这个键");

    // 候选：url 是 `downloadUrl ?: pageUrl` 的回退；可空键省掉
    let mut bare = candidate_of("krea2 Turbo Int8", ModelKind::Checkpoint);
    bare.sha256 = None;
    bare.size_bytes = None;
    bare.trained_words = Vec::new();
    bare.download_url = None;
    bare.downloads = None;
    bare.description = None;
    let mut result = result_of(
        vec![SourceOutcome::Ok {
            source: ModelSource::Civitai,
            candidates: vec![bare.clone()],
            detail: "通了，1 条".into(),
        }],
        vec![bare],
    );
    result.notes = vec!["这条没有校验值".into()];
    let value = to_find_model_json(&result);
    let entry = &value["candidates"][0];
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "name",
            "kind",
            "installInto",
            "baseModel",
            "url",
            "pageUrl",
            "fileName",
            "sources"
        ]
    );
    assert_eq!(
        entry["url"], "https://example.test/models/9",
        "没有直链就回退模型页"
    );
    assert!(entry.get("sha256").is_none(), "拿不到 sha256 就不写这个键");
    assert!(entry.get("sizeLabel").is_none());
    assert!(entry.get("downloads").is_none());
    assert_eq!(entry["sources"][0], "civitai");
    assert_eq!(value["notes"][0], "这条没有校验值");

    // 类型认不出 → 不写 installInto，但 note 要留着
    let mut unknown = candidate_of("z", ModelKind::Unknown);
    unknown.note = Some("diffusers 目录格式".into());
    let mut result = result_of(Vec::new(), vec![unknown.clone()]);
    result.sources = vec![SourceOutcome::Ok {
        source: ModelSource::HuggingFace,
        candidates: vec![unknown],
        detail: "通了，1 条".into(),
    }];
    let value = to_find_model_json(&result);
    let entry = &value["candidates"][0];
    assert!(
        entry.get("installInto").is_none(),
        "说不准就不要给一个目录：{entry}"
    );
    assert_eq!(entry["note"], "diffusers 目录格式");
    assert_eq!(entry["kind"], "unknown");
    assert_eq!(value["count"], 1);
}

// -----------------------------------------------------------------------
//  真 socket（进程内假三家，**不出网**）
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
    body: String,
}

/// `routes` 的键：精确路径，或 `*` 结尾表示前缀（`*/repo/files` 用得上）。
#[derive(Clone)]
struct Fake {
    routes: Arc<HashMap<String, (u16, String)>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn fake_models(routes: Vec<(&str, u16, &str)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake {
        routes: Arc::new(
            routes
                .into_iter()
                .map(|(path, status, body)| (path.to_string(), (status, body.to_string())))
                .collect(),
        ),
        seen: seen.clone(),
    };
    let app = Router::new()
        .fallback(axum::routing::any(
            |AxumState(f): AxumState<Fake>, request: Request| async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
                    .await
                    .unwrap_or_default();
                let path = parts.uri.path().to_string();
                f.seen.lock().unwrap().push(Seen {
                    method: parts.method.to_string(),
                    path: path.clone(),
                    query: parts.uri.query().unwrap_or_default().to_string(),
                    body: String::from_utf8_lossy(&bytes).to_string(),
                });
                let hit = f.routes.get(&path).cloned().or_else(|| {
                    f.routes
                        .iter()
                        .find(|(key, _)| {
                            key.ends_with('*') && path.starts_with(&key[..key.len() - 1])
                        })
                        .map(|(_, value)| value.clone())
                });
                let (status, body) = hit.unwrap_or((404, "{}".to_string()));
                Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
                    .into_response()
            },
        ))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假三家");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("假三家服务");
    });
    std::mem::forget(handle);
    (format!("http://{addr}"), seen)
}

fn finder(base: &str) -> HttpModelFinder {
    HttpModelFinder {
        civitai: Some(CivitaiClient::new(base, base, 10, None).expect("Civitai 客户端")),
        hf_token: None,
        hf_base: base.to_string(),
        ms_base: base.to_string(),
        timeout: Duration::from_secs(10),
        per_source: PER_SOURCE_LIMIT,
    }
}

fn sha(ch: char) -> String {
    ch.to_string().repeat(64)
}

fn inventory_of(name: &str, folder: &str) -> Inventory {
    Inventory {
        source: Source::ObjectInfo,
        models: vec![LocalModel {
            name: name.to_string(),
            folder: folder.to_string(),
            path: None,
            bytes: None,
            visible_to_instance: true,
        }],
        models_dir: None,
        truncated: false,
        disk_only: Vec::new(),
        merged: false,
    }
}

fn civitai_body() -> String {
    format!(
        r#"{{"items":[{{"id":9,"name":"krea2 Turbo Int8","type":"Checkpoint","nsfw":false,
          "stats":{{"downloadCount":10,"thumbsUpCount":2}},
          "description":"大底模",
          "modelVersions":[{{"baseModel":"SDXL 1.0","trainedWords":["krea2"],
            "files":[{{"name":"krea2_turbo_int8.safetensors","primary":true,"sizeKB":14145495.0,
                      "hashes":{{"SHA256":"{}","AutoV2":"ABCDEF1234"}},
                      "downloadUrl":"https://civitai.com/api/download/models/9"}}]}}]}}]}}"#,
        sha('c')
    )
}

/// HF 检索（**顶层是数组**）+ 目录（同样是数组，最大的是 `loras/…`）。
fn hf_search_body() -> String {
    r#"[{"id":"krea/Krea-2-Turbo","modelId":"krea/Krea-2-Turbo","downloads":74573,
       "pipeline_tag":"text-to-image","library_name":"diffusers",
       "tags":["diffusers","safetensors","base_model:krea/Krea-2-Raw"]}]"#
        .to_string()
}

fn hf_tree_body() -> String {
    format!(
        r#"[{{"type":"file","path":"README.md","size":1000,"oid":"deadbeef"}},
         {{"type":"file","path":"loras/krea2-turbo.safetensors","size":400000000,
          "lfs":{{"oid":"{}","size":400000000,"pointerSize":131}}}},
         {{"type":"file","path":"vae/ae.safetensors","size":300000000,
          "lfs":{{"oid":"{}","size":300000000}}}}]"#,
        sha('b'),
        sha('e')
    )
}

fn modelscope_body() -> String {
    r#"{"Code":200,"Data":{"Model":{"Models":[
      {"Id":653643,"Path":"circlestone-labs/Anima","Name":"Anima","Downloads":63821,
       "BaseModel":["nvidia/Cosmos-Predict2-2B-Text2Image"],"Tasks":["text-to-image"],
       "ModelInfos":{"safetensor":{"model_size":19542562803,"tensor_type":["BF16"]}}}]}}}"#
        .to_string()
}

fn modelscope_files_body() -> String {
    format!(
        r#"{{"Code":200,"Data":{{"Files":[
          {{"Type":"tree","Path":"split_files","Name":"split_files","Size":0,"Sha256":""}},
          {{"Type":"blob","Path":"split_files/diffusion_models/anima-turbo-v1.1.safetensors",
           "Name":"anima-turbo-v1.1.safetensors","Size":26283332608,"Sha256":"{}"}},
          {{"Type":"blob","Path":"README.md","Name":"README.md","Size":24848,
           "Sha256":"{}"}}]}}}}"#,
        sha('d'),
        sha('f')
    )
}

/// 三家一起看：**顺序 / 字段 / sha256 / installInto / 可达性**。
///
/// sha256 从哪儿来是这条用例的核心：
///  · Civitai = 版本 hash（`files[primary].hashes.SHA256`）；
///  · HF = **LFS oid**（`tree` 接口里的 `lfs.oid`）；
///  · ModelScope = 目录接口里那个 `Sha256` 字段。
#[tokio::test]
async fn 三源按顺序查到且_sha256_与_install_into_各自取对() {
    let (base, seen) = fake_models(vec![
        ("/api/v1/models/*", 200, &modelscope_files_body()),
        ("/api/v1/dolphin/models", 200, &modelscope_body()),
        ("/api/models", 200, &hf_search_body()),
        (
            "/api/models/krea/Krea-2-Turbo/tree/main",
            200,
            &hf_tree_body(),
        ),
        ("/api/v1/models", 200, &civitai_body()),
    ])
    .await;

    let result = finder(&base)
        .search(ModelQuery {
            text: "krea2 turbo".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    assert!(result.online);

    // 顺序固定：Civitai → HuggingFace → ModelScope
    assert_eq!(
        result
            .sources
            .iter()
            .map(|s| s.source())
            .collect::<Vec<_>>(),
        vec![
            ModelSource::Civitai,
            ModelSource::HuggingFace,
            ModelSource::ModelScope
        ]
    );
    assert!(
        result.sources.iter().all(|s| s.state() == Reachability::Ok),
        "{:?}",
        result.sources
    );
    assert_eq!(
        result.candidates.len(),
        3,
        "三个源各一条：{:?}",
        result.candidates
    );

    let civitai = result
        .candidates
        .iter()
        .find(|c| c.source == ModelSource::Civitai)
        .unwrap();
    assert_eq!(
        civitai.sha256.as_deref(),
        Some(sha('c').as_str()),
        "Civitai 的 sha256 = 版本 hash"
    );
    assert_eq!(civitai.install_into(), Some("checkpoints"));
    assert_eq!(
        civitai.file_name.as_deref(),
        Some("krea2_turbo_int8.safetensors")
    );
    assert_eq!(civitai.base_model.as_deref(), Some("SDXL 1.0"));

    let hf = result
        .candidates
        .iter()
        .find(|c| c.source == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(
        hf.sha256.as_deref(),
        Some(sha('b').as_str()),
        "HF 的 sha256 = LFS oid"
    );
    // 路径里写着 loras → installInto 必须是 loras（不是 pipeline_tag 推的 diffusion）
    assert_eq!(
        hf.install_into(),
        Some("loras"),
        "路径线索比 pipeline_tag 准：{:?}",
        hf.file_name
    );
    assert_eq!(hf.name, "krea/Krea-2-Turbo");
    assert_eq!(hf.base_model.as_deref(), Some("krea/Krea-2-Raw"));
    // 直链要带上**仓库里的路径**（这个文件在 loras/ 子目录里，少一段就是 404）
    assert!(
        hf.download_url
            .as_deref()
            .unwrap()
            .contains("/resolve/main/loras/krea2-turbo.safetensors"),
        "{:?}",
        hf.download_url
    );

    let ms = result
        .candidates
        .iter()
        .find(|c| c.source == ModelSource::ModelScope)
        .unwrap();
    assert_eq!(
        ms.sha256.as_deref(),
        Some(sha('d').as_str()),
        "ModelScope 的 sha256 = 仓库文件的 Sha256"
    );
    assert_eq!(
        ms.install_into(),
        Some("diffusion_models"),
        "split_files/diffusion_models 是路径线索"
    );
    assert_eq!(ms.name, "Anima");
    assert_eq!(
        ms.base_model.as_deref(),
        Some("nvidia/Cosmos-Predict2-2B-Text2Image")
    );

    // 请求侧：三家**顺序**发生，且 ModelScope 的检索是 **PUT**（GET 会 404）
    let hits = seen.lock().unwrap().clone();
    let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/api/v1/models",
            "/api/models",
            "/api/models/krea/Krea-2-Turbo/tree/main",
            "/api/v1/dolphin/models",
            "/api/v1/models/circlestone-labs/Anima/repo/files",
        ],
        "顺序 = Civitai → HF → ModelScope，每个源自己那几跳依次发生"
    );
    assert_eq!(hits[3].method, "PUT", "魔搭的检索接口是 PUT");
    assert!(
        hits[3].body.contains("\"Name\":\"krea2 turbo\""),
        "{}",
        hits[3].body
    );
    assert!(
        hits[3].body.contains("\"PageSize\":4"),
        "每个源最多 4 条：{}",
        hits[3].body
    );
    // HF 检索的查询串：`search=` 编码 + `sort=downloads`（Kotlin 那四个键）
    assert!(
        hits[1].query.contains("search=krea2+turbo"),
        "{}",
        hits[1].query
    );
    assert!(
        hits[1].query.contains("sort=downloads"),
        "{}",
        hits[1].query
    );
    assert!(hits[1].query.contains("direction=-1"), "{}", hits[1].query);
    assert!(hits[1].query.contains("limit=4"), "{}", hits[1].query);
    // 魔搭仓库全名是 `Path/Name`
    assert!(paths[4].contains("circlestone-labs/Anima"), "{}", paths[4]);
    assert!(
        hits[4].query.contains("Revision=master"),
        "{}",
        hits[4].query
    );
}

/// 顺序约束：**先查本机，再查外网** —— 命中时不发送任何请求。
///
/// 命中的判据是"同名"，而且**不能用纯子串**：`anima` 不该命中 `anima-turbo`。
#[tokio::test]
async fn 本机同名时一个请求都不发并把三个源标成没去查() {
    let (base, seen) = fake_models(vec![("/api/v1/models", 200, &civitai_body())]).await;
    let finder = finder(&base);

    let hit = finder
        .search(ModelQuery {
            text: "krea2TurboInt8.safetensors".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: Some(inventory_of("krea2_turbo_int8.safetensors", "checkpoints")),
        })
        .await
        .unwrap();
    assert!(
        hit.local_hit(),
        "分词后同名要算命中（写法不同也是同一个模型）"
    );
    assert!(!hit.online, "命中本机就不该出网");
    assert!(seen.lock().unwrap().is_empty(), "一个外部请求都不该发");
    assert_eq!(hit.sources.len(), 3, "三个源照样要各占一行");
    assert!(
        hit.sources
            .iter()
            .all(|s| s.state() == Reachability::Skipped),
        "{:?}",
        hit.sources
    );
    assert!(
        hit.notes
            .iter()
            .any(|n| n.contains("没有") && n.contains("外网")),
        "{:?}",
        hit.notes
    );
    assert_eq!(hit.local_source, Source::ObjectInfo);
    assert_eq!(hit.local_matches.len(), 1);

    // 边界：`anima` 不该把 `anima-turbo` 当成"本机已有"
    let other = finder
        .search(ModelQuery {
            text: "anima".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: Some(inventory_of("anima-turbo.safetensors", "checkpoints")),
        })
        .await
        .unwrap();
    assert!(!other.local_hit(), "子串命中不是同名 —— 那是另一个模型");
    assert!(!seen.lock().unwrap().is_empty(), "没命中就该真的去查");
}

#[tokio::test]
async fn online_false_时只查本机且不碰外网() {
    let (base, seen) = fake_models(vec![]).await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "随便什么".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: false,
            inventory: None,
        })
        .await
        .unwrap();
    assert!(!result.online);
    assert!(seen.lock().unwrap().is_empty());
    assert!(result
        .sources
        .iter()
        .all(|s| s.state() == Reachability::Skipped));
    assert_eq!(result.local_source, Source::None);
    assert!(
        result.notes.iter().any(|n| n.contains("只查了本机")),
        "{:?}",
        result.notes
    );
    assert!(result
        .sources
        .iter()
        .all(|s| s.detail() == "没查：本次只查了本机"));
}

/// 本机清单来源要**如实**带进结果（`object_info` 与 `scan` 不是一回事）。
#[tokio::test]
async fn 本机清单来源如实带进结果() {
    let (base, _) = fake_models(vec![]).await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: false,
            inventory: Some(inventory_of("a.safetensors", "loras")),
        })
        .await
        .unwrap();
    assert_eq!(result.local_source, Source::ObjectInfo);
    assert_eq!(result.local_source.wire(), "object_info");
}

/// HF 的 401 / 403 **可区分**：需要登录 vs gated（"AI 报告可下载、实际无法下载"的回归防线）。
#[tokio::test]
async fn hf_的_401_与_403_分别落到_needs_login_与_gated() {
    let (base, _) = fake_models(vec![("/api/models", 401, r#"{"error":"unauthorized"}"#)]).await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .sources
        .iter()
        .find(|s| s.source() == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(hf.state(), Reachability::NeedsLogin, "{}", hf.detail());
    assert!(
        hf.detail().contains("登录") || hf.detail().contains("令牌"),
        "{}",
        hf.detail()
    );

    let (base, _) = fake_models(vec![("/api/models", 403, r#"{"error":"gated"}"#)]).await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .sources
        .iter()
        .find(|s| s.source() == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(hf.state(), Reachability::Gated, "{}", hf.detail());
    assert!(
        hf.detail().contains("同意条款"),
        "gated 要说清「用户得自己去点同意」：{}",
        hf.detail()
    );
}

/// 连不上 → `needs_proxy`，而且要写清出网路径与"别反复重试"。
#[tokio::test]
async fn 连不上时归到_needs_proxy_并写清出网路径与不要反复重试() {
    // 127.0.0.1 上一个**没有服务**的端口：连接会被立刻拒绝
    let mut finder = finder("http://127.0.0.1:1");
    finder.timeout = Duration::from_secs(3);
    let result = finder
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .sources
        .iter()
        .find(|s| s.source() == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(hf.state(), Reachability::NeedsProxy, "{}", hf.detail());
    assert!(
        hf.detail().contains("127.0.0.1:7890"),
        "要告诉用户代理怎么设：{}",
        hf.detail()
    );
    assert!(hf.detail().contains("不要反复重试"), "{}", hf.detail());
    // "没查到"与"没查成"要分开：三个源全挂时正文必须说出来
    let text = render_find_model(&result, None);
    assert!(text.contains("ModelScope=needs_proxy"), "{text}");
    assert!(text.contains("两件事要分开说"), "{text}");
}

/// 一个源挂了**不影响**别的源（"逐个报可达性"的前提）。
#[tokio::test]
async fn 一个源连不上另外两个照常出结果() {
    let (base, _) = fake_models(vec![
        ("/api/v1/models/*", 200, &modelscope_files_body()),
        ("/api/v1/dolphin/models", 200, &modelscope_body()),
        ("/api/v1/models", 200, &civitai_body()),
    ])
    .await;
    let mut finder = finder(&base);
    finder.hf_base = "http://127.0.0.1:1".to_string();
    finder.timeout = Duration::from_secs(3);
    let result = finder
        .search(ModelQuery {
            text: "krea2 turbo".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    assert_eq!(result.sources.len(), 3);
    assert_eq!(result.sources[0].state(), Reachability::Ok);
    assert_eq!(result.sources[1].state(), Reachability::NeedsProxy);
    assert_eq!(
        result.sources[2].state(),
        Reachability::Ok,
        "{}",
        result.sources[2].detail()
    );
    assert!(result
        .candidates
        .iter()
        .any(|c| c.source == ModelSource::Civitai));
    assert!(result
        .candidates
        .iter()
        .any(|c| c.source == ModelSource::ModelScope));
}

/// 上游没给 sha256 时**不编**，而且要在 notes 里如实说明。
#[tokio::test]
async fn 上游没给_sha256_时不编且如实说明() {
    let body = r#"{"items":[{"id":9,"name":"没给校验值的模型","type":"LORA","nsfw":false,
      "stats":{"downloadCount":10,"thumbsUpCount":2},"description":"大底模",
      "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":[],
        "files":[{"name":"a.safetensors","primary":true,"sizeKB":1024.0,
                  "downloadUrl":"https://civitai.com/api/download/models/9"}]}]}]}"#;
    let (base, _) = fake_models(vec![
        ("/api/v1/models", 200, body),
        ("/api/models", 200, "[]"),
    ])
    .await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "krea2 turbo".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let candidate = result.candidates.first().unwrap();
    assert_eq!(
        candidate.sha256, None,
        "上游没给就是 None —— 不许编一个 64 位十六进制"
    );
    assert!(
        result.notes.iter().any(|n| n.contains("sha256")),
        "要如实说'这条没有校验值'：{:?}",
        result.notes
    );
    let text = render_find_model(&result, None);
    assert!(
        text.contains("（这一条上游没给 sha256，下完没法校验）"),
        "{text}"
    );
    assert!(text.contains("\n· 本次没有一条带 sha256"), "{text}");
}

/// HF 非 LFS 文件（oid 不是 64 位）**不当成 sha256** 报出去。
#[tokio::test]
async fn hf_非_lfs_文件不当成_sha256() {
    let tree = r#"[{"type":"file","path":"krea2-turbo.safetensors","size":200000000,
      "lfs":{"oid":"not-a-sha256","size":200000000}}]"#;
    let (base, _) = fake_models(vec![
        ("/api/models", 200, &hf_search_body()),
        ("/api/models/krea/Krea-2-Turbo/tree/main", 200, tree),
    ])
    .await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .candidates
        .iter()
        .find(|c| c.source == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(hf.sha256, None, "不是 64 位十六进制就不是 sha256");
    assert_eq!(hf.name, "krea/Krea-2-Turbo");
}

/// diffusers 目录格式：**不给目录**，并给出那条"别把它当能用的底模"的说明。
#[tokio::test]
async fn diffusers_仓库不给_install_into_但给说明() {
    let body = r#"[{"modelId":"krea/Krea-2-Turbo","library_name":"diffusers",
      "pipeline_tag":"text-to-image","tags":["diffusers","base_model:x"],
      "downloads":100}]"#;
    let tree = r#"[{"type":"file","path":"turbo.safetensors","size":24480000000}]"#;
    let (base, _) = fake_models(vec![
        ("/api/models", 200, body),
        ("/api/models/krea/Krea-2-Turbo/tree/main", 200, tree),
    ])
    .await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .candidates
        .iter()
        .find(|c| c.source == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(
        hf.kind,
        ModelKind::Unknown,
        "diffusers 的整包文件不能当成底模"
    );
    assert_eq!(hf.install_into(), None);
    assert!(
        hf.note.as_deref().unwrap().contains("diffusers 目录格式"),
        "{:?}",
        hf.note
    );
    let text = render_find_model(&result, None);
    assert!(
        text.contains("该放进：**这个文件不一定能直接放进 ComfyUI**"),
        "{text}"
    );
    assert!(
        text.contains("⚠ 这个仓库是 **diffusers 目录格式**"),
        "{text}"
    );
}

/// 顶层不是数组时**如实报解析错**（第一版就是在这里把 HF 判成"挂了"）。
#[tokio::test]
async fn hf_顶层不是数组时如实报解析错() {
    let (base, _) = fake_models(vec![("/api/models", 200, r#"{"error":"not an array"}"#)]).await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let hf = result
        .sources
        .iter()
        .find(|s| s.source() == ModelSource::HuggingFace)
        .unwrap();
    assert_eq!(hf.state(), Reachability::Error);
    assert!(hf.detail().contains("顶层不是数组"), "{}", hf.detail());
}

#[tokio::test]
async fn 空查询词在真实实现那一层直接报_invalid_argument() {
    let (base, _) = fake_models(vec![]).await;
    let err = finder(&base)
        .search(ModelQuery {
            text: "   ".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

/// `civitai: None` 那一档（Kotlin 的 `CIVITAI_DISABLED`）只留给单测 —— 文案照抄。
#[tokio::test]
async fn 没有_civitai_客户端时它那一行如实报错() {
    let (base, _) = fake_models(vec![("/api/models", 200, "[]")]).await;
    let mut finder = finder(&base);
    finder.civitai = None;
    let result = finder
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: DEFAULT_LIMIT,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    let civitai = result
        .sources
        .iter()
        .find(|s| s.source() == ModelSource::Civitai)
        .unwrap();
    assert_eq!(civitai.state(), Reachability::Error);
    assert_eq!(
        civitai.detail(),
        "本次运行没有接入 Civitai 搜索客户端（CIVITAI_DISABLED）"
    );
}

/// `limit` 夹到 [1, 20]，且**合并之后**才截断（三个源各问 4 条）。
#[tokio::test]
async fn limit_夹到上限且合并之后才截断() {
    let (base, seen) = fake_models(vec![
        ("/api/v1/models", 200, &civitai_body()),
        ("/api/models", 200, "[]"),
        ("/api/v1/dolphin/models", 200, "{}"),
    ])
    .await;
    let result = finder(&base)
        .search(ModelQuery {
            text: "x".into(),
            kind: None,
            limit: 999,
            online: true,
            inventory: None,
        })
        .await
        .unwrap();
    // 每个源最多 4 条（不跟着 limit 放大）
    let hits = seen.lock().unwrap().clone();
    assert!(hits.iter().any(|h| h.query.contains("limit=4")), "{hits:?}");
    assert_eq!(result.candidates.len(), 1);
}
