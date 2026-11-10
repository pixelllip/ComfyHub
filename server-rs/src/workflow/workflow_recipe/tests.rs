//! `workflow_recipe` 的单元测试（主体代码在 [`workflow_recipe`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::comfy::comfy_models::{Inventory, LocalModel, Source};

// -----------------------------------------------------------------------
//  夹具小工具
// -----------------------------------------------------------------------

fn link(id: &str, slot: i64) -> Value {
    json!([id, slot])
}

fn node(class_type: &str, title: Option<&str>, inputs: Value) -> Value {
    let mut m = Map::new();
    if let Some(title) = title {
        m.insert("_meta".into(), json!({ "title": title }));
    }
    m.insert("class_type".into(), json!(class_type));
    m.insert("inputs".into(), inputs);
    Value::Object(m)
}

fn graph(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    let mut m = Map::new();
    for (id, n) in pairs {
        m.insert(id.to_string(), n);
    }
    m
}

fn graph_of(text: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(text)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn analyze(graph: &Map<String, Value>) -> Recipe {
    super::analyze(graph, None)
}

/// 一份最小的 t2i：底模 + 文本编码 + 空潜空间 + 采样 + 解码 + 存图。
fn t2i_graph(unet: &str) -> Map<String, Value> {
    graph(vec![
        (
            "1",
            node(
                "CheckpointLoaderSimple",
                Some("底模"),
                json!({ "ckpt_name": unet }),
            ),
        ),
        (
            "6",
            node(
                "CLIPTextEncode",
                Some("正面"),
                json!({ "clip": link("1", 1), "text": "a cat" }),
            ),
        ),
        (
            "7",
            node(
                "CLIPTextEncode",
                Some("负面"),
                json!({ "clip": link("1", 1), "text": "blurry" }),
            ),
        ),
        (
            "5",
            node(
                "EmptyLatentImage",
                None,
                json!({ "width": 1024, "height": 1024, "batch_size": 1 }),
            ),
        ),
        (
            "3",
            node(
                "KSampler",
                None,
                json!({
                    "model": link("1", 0), "positive": link("6", 0), "negative": link("7", 0),
                    "latent_image": link("5", 0), "seed": 42, "steps": 8, "cfg": 1.0,
                    "sampler_name": "euler", "scheduler": "simple", "denoise": 1.0
                }),
            ),
        ),
        (
            "8",
            node(
                "VAEDecode",
                None,
                json!({ "samples": link("3", 0), "vae": link("1", 2) }),
            ),
        ),
        (
            "9",
            node(
                "SaveImage",
                None,
                json!({ "images": link("8", 0), "filename_prefix": "x" }),
            ),
        ),
    ])
}

fn deps(info: Option<Map<String, Value>>) -> Deps<'static> {
    Deps {
        object_info: info,
        api_graph_of: Box::new(|_| None),
        prompt_name: Box::new(|_| None),
        find_prompt_id_by_run_key: Box::new(|_| None),
    }
}

// -----------------------------------------------------------------------
//  ① 骨架指纹
// -----------------------------------------------------------------------

#[test]
fn 骨架指纹对节点编号_模型名_参数值稳定() {
    let a = analyze(&t2i_graph("a.safetensors"));
    let b = analyze(&t2i_graph("b.safetensors"));
    assert_eq!(
        a.skeleton_hash, b.skeleton_hash,
        "换了底模不该换骨架（这正是'泛用'的定义）"
    );

    // 重新编号（把 6/7 换成 66/77）+ 改提示词与 seed：形状没变
    let renamed = graph(vec![
        (
            "1",
            node(
                "CheckpointLoaderSimple",
                None,
                json!({ "ckpt_name": "c.safetensors" }),
            ),
        ),
        (
            "66",
            node(
                "CLIPTextEncode",
                None,
                json!({ "clip": link("1", 1), "text": "完全不同的提示词" }),
            ),
        ),
        (
            "77",
            node(
                "CLIPTextEncode",
                None,
                json!({ "clip": link("1", 1), "text": "nope" }),
            ),
        ),
        (
            "5",
            node(
                "EmptyLatentImage",
                None,
                json!({ "width": 512, "height": 512, "batch_size": 2 }),
            ),
        ),
        (
            "3",
            node(
                "KSampler",
                None,
                json!({
                    "model": link("1", 0), "positive": link("66", 0), "negative": link("77", 0),
                    "latent_image": link("5", 0), "seed": 999, "steps": 30, "cfg": 7.0,
                    "sampler_name": "dpmpp_2m", "scheduler": "karras", "denoise": 0.8
                }),
            ),
        ),
        (
            "8",
            node(
                "VAEDecode",
                None,
                json!({ "samples": link("3", 0), "vae": link("1", 2) }),
            ),
        ),
        (
            "9",
            node("SaveImage", None, json!({ "images": link("8", 0) })),
        ),
    ]);
    assert_eq!(a.skeleton_hash, analyze(&renamed).skeleton_hash);
}

#[test]
fn 骨架指纹对结构变化敏感_也认重数() {
    let base = analyze(&t2i_graph("krea2_turbo_nvfp4.safetensors"));
    // 多一个 LoRA（结构变了）
    let mut with_lora = t2i_graph("krea2_turbo_nvfp4.safetensors");
    with_lora.insert(
        "20".into(),
        node(
            "LoraLoaderModelOnly",
            None,
            json!({
                "model": link("1", 0), "lora_name": "style.safetensors", "strength_model": 1.0
            }),
        ),
    );
    with_lora.insert(
        "3".into(),
        node(
            "KSampler",
            None,
            json!({
                "model": link("20", 0), "positive": link("6", 0), "negative": link("7", 0),
                "latent_image": link("5", 0), "seed": 42, "steps": 8, "cfg": 1.0,
                "sampler_name": "euler", "scheduler": "simple", "denoise": 1.0
            }),
        ),
    );
    assert_ne!(
        base.skeleton_hash,
        analyze(&with_lora).skeleton_hash,
        "多一个结构节点必须换指纹"
    );

    // 两个 LoRA 串起来 ≠ 一个
    let mut two_loras = with_lora.clone();
    two_loras.insert(
        "21".into(),
        node(
            "LoraLoaderModelOnly",
            None,
            json!({
                "model": link("20", 0), "lora_name": "b.safetensors", "strength_model": 0.6
            }),
        ),
    );
    two_loras.insert(
        "3b".into(),
        node(
            "KSampler",
            None,
            json!({
                "model": link("21", 0), "positive": link("6", 0), "negative": link("7", 0),
                "latent_image": link("5", 0)
            }),
        ),
    );
    assert_ne!(
        analyze(&with_lora).skeleton_hash,
        analyze(&two_loras).skeleton_hash,
        "重数要进指纹（一/两个 LoRA 不是同一个形状）"
    );
}

#[test]
fn 界面节点与纯显示节点不进指纹() {
    let base = analyze(&t2i_graph("krea2_turbo_nvfp4.safetensors"));
    let mut noisy = t2i_graph("krea2_turbo_nvfp4.safetensors");
    for (id, value) in [
        ("90", node("Note", None, json!({ "text": "我的笔记" }))),
        (
            "91",
            node("MarkdownNote", None, json!({ "text": "# 标题" })),
        ),
        (
            "92",
            node("Reroute", None, json!({ "input": link("9", 0) })),
        ),
        ("93", node("GetNode", None, json!({ "id": "x" }))),
        (
            "94",
            node("Bookmark (rgthree)", None, json!({ "bookmark": "x" })),
        ),
        ("95", node("Label (rgthree)", None, json!({ "text": "x" }))),
        (
            "96",
            node("PreviewImage", None, json!({ "images": link("8", 0) })),
        ),
        (
            "97",
            node(
                "Image Comparer (rgthree)",
                None,
                json!({ "image_a": link("8", 0) }),
            ),
        ),
    ] {
        noisy.insert(id.to_string(), value);
    }
    assert_eq!(
        base.skeleton_hash,
        analyze(&noisy).skeleton_hash,
        "Note / Reroute / Bookmark / Preview 这类界面节点不该改变骨架（113 节点的那份里它们占三分之一）"
    );
    // 但节点数是**如实**的（报告里要能看出"这份文件比那份胖"）
    assert_eq!(base.node_count, 7);
    assert_eq!(analyze(&noisy).node_count, 15);
}

// -----------------------------------------------------------------------
//  ② 模型槽位
// -----------------------------------------------------------------------

#[test]
fn 模型槽位按确定表认出角色_且不在旋钮里重复() {
    let g = graph(vec![
        (
            "73",
            node(
                "UNETLoader",
                Some("底模"),
                json!({ "unet_name": "krea2_turbo_bf16.safetensors", "weight_dtype": "default" }),
            ),
        ),
        (
            "56",
            node(
                "CLIPLoader",
                Some("文本编码器"),
                json!({ "clip_name": "qwen3vl_4b.safetensors", "type": "krea2", "device": "default" }),
            ),
        ),
        (
            "91",
            node(
                "VAELoader",
                None,
                json!({ "vae_name": "qwen_image_vae.safetensors" }),
            ),
        ),
        (
            "71",
            node(
                "LoraLoaderModelOnly",
                None,
                json!({
                    "model": link("73", 0), "lora_name": "identity.safetensors",
                    "strength_model": 1.0
                }),
            ),
        ),
        (
            "60",
            node(
                "TextInput_",
                None,
                json!({ "text": "把这个人换成正在跑步的姿势" }),
            ),
        ),
        (
            "72",
            node(
                "LoadImage",
                Some("参考图"),
                json!({ "image": "clipspace/abc.png [input]" }),
            ),
        ),
    ]);
    let r = analyze(&g);

    // 槽位按**节点号**排（`ComfyModels.requirements` 就是这么给的）：
    // 顺序稳定比"按文件顺序"更重要 —— 同一份图学两次，报告要逐字一样
    let roles: Vec<(&str, &str)> = r
        .slots
        .iter()
        .map(|s| (s.node_id.as_str(), s.role.as_str()))
        .collect();
    assert_eq!(
        roles,
        vec![
            ("56", "text_encoders"),
            ("71", "loras"),
            ("73", "diffusion_models"),
            ("91", "vae"),
        ],
        "角色取自 ComfyModels 那张确定表（folder），不新建一套；且按节点号升序"
    );
    let slot = r.slots.iter().find(|s| s.node_id == "73").unwrap();
    assert_eq!(slot.override_key(), "73.unet_name");
    assert_eq!(slot.file_name, "krea2_turbo_bf16.safetensors");
    assert_eq!(
        slot.title.as_deref(),
        Some("底模"),
        "节点标题只是可读性，不参与判定"
    );
    // 槽位键不许在旋钮里再来一份（两边重复会让'对不上'更难查）
    let knob_keys: HashSet<String> = r.knobs.iter().map(|k| k.override_key()).collect();
    for slot in &r.slots {
        assert!(
            !knob_keys.contains(&slot.override_key()),
            "槽位 {} 不该出现在旋钮里",
            slot.override_key()
        );
    }
    // 但加载器上那些**模型族相关**的开关要留下（type=krea2 换模型族时必须改）
    assert!(r
        .knobs
        .iter()
        .any(|k| k.override_key() == "56.type" && k.kind == KnobKind::Loader));
    // 连线型输入不是旋钮
    assert!(
        !knob_keys.contains("71.model"),
        "连线（数组）不是可覆盖的字面量"
    );
}

// -----------------------------------------------------------------------
//  ③ 任务分类
// -----------------------------------------------------------------------

#[test]
fn 任务分类_七档各有判据() {
    // t2i：没有任何图像输入
    assert_eq!(analyze(&t2i_graph("a.safetensors")).task, Task::T2i);

    // i2i：有 LoadImage
    let mut i2i = t2i_graph("a.safetensors");
    i2i.insert(
        "30".into(),
        node("LoadImage", None, json!({ "image": "a.png" })),
    );
    i2i.insert(
        "31".into(),
        node(
            "VAEEncode",
            None,
            json!({ "pixels": link("30", 0), "vae": link("1", 2) }),
        ),
    );
    i2i.insert(
        "3".into(),
        node(
            "KSampler",
            None,
            json!({ "latent_image": link("31", 0), "model": link("1", 0) }),
        ),
    );
    assert_eq!(analyze(&i2i).task, Task::I2i);

    // inpaint：有遮罩链
    let mut inpaint = i2i.clone();
    inpaint.insert(
        "12".into(),
        node(
            "LoadImageMask",
            None,
            json!({ "image": "m.png", "channel": "red" }),
        ),
    );
    inpaint.insert(
        "24".into(),
        node(
            "SetLatentNoiseMask",
            None,
            json!({ "samples": link("31", 0), "mask": link("12", 0) }),
        ),
    );
    inpaint.insert(
        "3".into(),
        node(
            "KSampler",
            None,
            json!({ "latent_image": link("24", 0), "model": link("1", 0) }),
        ),
    );
    assert_eq!(analyze(&inpaint).task, Task::Inpaint);
    assert!(analyze(&inpaint).has_mask);

    // edit：参考图编辑链 —— **即便同时有遮罩链**也必须判 edit（Krea 2 Identity Edit 就是两者都有）
    let mut edit = inpaint.clone();
    edit.insert(
        "58".into(),
        node(
            "Krea2EditModelPatch",
            None,
            json!({ "model": link("1", 0), "latent": link("31", 0) }),
        ),
    );
    edit.insert(
        "60".into(),
        node(
            "TextEncodeKrea2OstrisEdit",
            None,
            json!({ "clip": link("1", 1), "text": "换成跑步" }),
        ),
    );
    assert_eq!(
        analyze(&edit).task,
        Task::Edit,
        "edit 必须优先于 inpaint：归成 inpaint 的话 comfy_list_recipes(task=edit) 永远找不到用户的图像编辑工作流"
    );

    // video：有 SaveVideo（输出即事实）
    let video = graph(vec![
        ("1", node("LoadImage", None, json!({ "image": "a.png" }))),
        (
            "2",
            node(
                "MiniMaxH3ImageToVideo",
                None,
                json!({ "image": link("1", 0), "prompt": "走起来" }),
            ),
        ),
        (
            "3",
            node("SaveVideo", None, json!({ "video": link("2", 0) })),
        ),
    ]);
    assert_eq!(analyze(&video).task, Task::Video);

    // 视频模型但**只出一张图**：不许判成 video（光看节点名就会错）
    let video_model_image_out = graph(vec![
        (
            "1",
            node("MiniMaxH3Sampler", None, json!({ "prompt": "一只猫" })),
        ),
        (
            "2",
            node("VAEDecode", None, json!({ "samples": link("1", 0) })),
        ),
        (
            "3",
            node("SaveImage", None, json!({ "images": link("2", 0) })),
        ),
    ]);
    assert_eq!(analyze(&video_model_image_out).task, Task::T2i);

    // audio：音频输出
    let audio = graph(vec![(
        "1",
        node("SaveAudioAdvanced", None, json!({ "audio": "x" })),
    )]);
    assert_eq!(analyze(&audio).task, Task::Audio);

    // upscale：只有放大模型、没有采样器
    let upscale = graph(vec![
        ("1", node("LoadImage", None, json!({ "image": "a.png" }))),
        (
            "2",
            node(
                "UpscaleModelLoader",
                None,
                json!({ "model_name": "4x.pth" }),
            ),
        ),
        (
            "3",
            node(
                "ImageUpscaleWithModel",
                None,
                json!({ "upscale_model": link("2", 0), "image": link("1", 0) }),
            ),
        ),
        (
            "4",
            node("SaveImage", None, json!({ "images": link("3", 0) })),
        ),
    ]);
    assert_eq!(analyze(&upscale).task, Task::Upscale);

    // unknown：认不出（不能硬塞一档）
    let unknown = graph(vec![(
        "1",
        node("PrimitiveStringMultiline", None, json!({ "value": "随便" })),
    )]);
    let r = analyze(&unknown);
    assert_eq!(r.task, Task::Unknown);
    assert!(r.warnings.iter().any(|w| w.contains("认不出")));
}

#[test]
fn resize_image_mask_node_里有_mask_但它不是_inpaint() {
    let g = graph(vec![
        ("1", node("LoadImage", None, json!({ "image": "a.png" }))),
        (
            "2",
            node(
                "ResizeImageMaskNode",
                None,
                json!({ "image": link("1", 0), "width": 512 }),
            ),
        ),
        (
            "3",
            node("SaveImage", None, json!({ "images": link("2", 0) })),
        ),
    ]);
    assert_eq!(analyze(&g).task, Task::I2i);
}

// -----------------------------------------------------------------------
//  ④ 旋钮与如实警告
// -----------------------------------------------------------------------

#[test]
fn 旋钮按种类分组_值可截断_计数如实() {
    let r = analyze(&t2i_graph("a.safetensors"));
    let has = |kind: KnobKind, key: &str| {
        r.knobs
            .iter()
            .any(|k| k.kind == kind && k.override_key() == key)
    };
    assert!(
        has(KnobKind::Prompt, "6.text"),
        "文本编码的 text 要判成提示词"
    );
    assert!(has(KnobKind::Sampling, "3.steps"));
    assert!(has(KnobKind::Sampling, "3.sampler_name"));
    assert!(has(KnobKind::Seed, "3.seed"));
    assert!(has(KnobKind::Size, "5.width"));
    // 提示词排在最前面（模型最先要看的就是它）
    assert_eq!(r.knobs.first().unwrap().kind, KnobKind::Prompt);

    // 超长值只留前 MAX_KNOB_VALUE 个字符（否则一份 2KB 的自然语言提示词就把结果撑满了）
    let long = "x".repeat(MAX_KNOB_VALUE + 50);
    let huge = analyze(&graph(vec![(
        "1",
        node(
            "CLIPTextEncode",
            None,
            json!({ "clip": link("1", 1), "text": long }),
        ),
    )]));
    let knob = huge
        .knobs
        .iter()
        .find(|k| k.kind == KnobKind::Prompt)
        .unwrap();
    assert!(knob.value.ends_with('…'), "超长值要带截断标记");
    assert_eq!(knob.value.chars().count(), MAX_KNOB_VALUE + 1);
}

#[test]
fn 拿不到_object_info_与空图都要如实说() {
    let r = analyze(&t2i_graph("a.safetensors"));
    assert!(
        r.warnings.iter().any(|w| w.contains("/object_info")),
        "没有 objectInfo 时必须说'自定义加载器的槽位可能漏报'，不许装作查全了"
    );
    // 给了 objectInfo（哪怕是空表）就不再报这条
    let with_info = super::analyze(&t2i_graph("a.safetensors"), Some(&HashMap::new()));
    assert!(!with_info
        .warnings
        .iter()
        .any(|w| w.contains("/object_info")));

    let empty = analyze(&Map::new());
    assert_eq!(empty.task, Task::Unknown);
    assert!(empty.warnings.iter().any(|w| w.contains("空图")));
    assert!(empty.slots.is_empty() && empty.knobs.is_empty());
}

// -----------------------------------------------------------------------
//  ⑤ render / toJson
// -----------------------------------------------------------------------

#[test]
fn render_里能直接看到改哪个键_to_json_键集合稳定() {
    let r = analyze(&t2i_graph("krea2_turbo_nvfp4.safetensors"));
    let text = r.render(Some("krea2_edit"));
    assert!(text.contains("【工作流配方】krea2_edit"));
    assert!(text.contains("任务：t2i"));
    assert!(text.contains(&format!("骨架：{}", r.skeleton_hash)));
    assert!(
        text.contains("1.ckpt_name = krea2_turbo_nvfp4.safetensors"),
        "{text}"
    );
    assert!(text.contains("提示词："), "{text}");
    assert!(text.contains("#9 SaveImage（image）"));

    let json = r.to_json();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "classCount",
            "edgeCount",
            "edges",
            "hasMask",
            "imageInputs",
            "knobs",
            "knobsTruncated",
            "nodeClasses",
            "nodeCount",
            "outputs",
            "recipeVersion",
            "skeletonHash",
            "slots",
            "task",
            "warnings",
        ],
        "少一个键 = 界面上少一块内容"
    );
    assert_eq!(json["recipeVersion"], json!(VERSION));
}

// -----------------------------------------------------------------------
//  真文件
// -----------------------------------------------------------------------
//
// 2026-10-28：仓库**不再随包带模板工作流**（模板一律用各模型在 ComfyUI 模板库里的
// 标准工作流），原来那三条"拿仓库里的 inpaint krea2 / anima 文件当夹具"的用例一并删除。
// "从真文件学"这条路仍由下面「学一份 inpaint 图能落库并按 hash 幂等」覆盖 ——
// 它把合成 API 图写到 `testpaths::scratch()`，仍然走磁盘文件那条入口。

// -----------------------------------------------------------------------
//  仓储（`WorkflowRecipeRepoTest.kt`）
// -----------------------------------------------------------------------

/// 内存库 + **真 DDL**（`db/schema.sqlite.sql` 是唯一真源）——
/// 用测试里另写的一份建表语句就不是在测真库了（`source_ref` 唯一索引这类承重约束会凭空消失）。
fn conn() -> Connection {
    let conn = crate::infra::db::open_memory().unwrap();
    conn.execute_batch(crate::infra::db::schema_sql())
        .expect("建 schema");
    crate::infra::db::assert_ready(&conn).unwrap();
    conn
}

fn sample_recipe() -> Recipe {
    analyze(&graph_of(
        r#"{
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"a.safetensors"}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"clip":["1",1],"text":"a cat"}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":1024,"height":1024,"batch_size":1}},
          "3": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["6",0],"latent_image":["5",0],
                 "seed":42,"steps":8,"cfg":1.0,"sampler_name":"euler","scheduler":"simple","denoise":1.0}},
          "8": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
          "9": {"class_type":"SaveImage","inputs":{"images":["8",0]}}
        }"#,
    ))
}

fn sample_write(
    name: &str,
    source_ref: Option<&str>,
    prompt_id: Option<i64>,
    complete: bool,
    gaps: &[String],
) -> Write {
    write_of(
        &sample_recipe(),
        WriteOf {
            name,
            source_kind: "file",
            source_ref,
            source_path: Some(&format!("D:/wf/{name}.json")),
            prompt_id,
            complete,
            gaps,
            extra_warnings: &[],
            notes: None,
        },
    )
}

#[test]
fn 同一份来源学两次只有一行_且_id_不变() {
    let conn = conn();
    let (first, created_first) = save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap();
    assert!(created_first);
    let (second, created_second) = save_row(
        &conn,
        &sample_write("改了个名字", Some("file:abc"), None, true, &[]),
    )
    .unwrap();
    assert_eq!(first, second, "同一份来源必须复用同一行");
    assert!(!created_second);
    assert_eq!(
        list(&conn, None, None, None, false, MAX_LIST)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(get(&conn, first).unwrap().unwrap().name, "改了个名字");
}

#[test]
fn 重新学一份会整行更新_不留旧槽位() {
    let conn = conn();
    let id = save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap()
    .0;
    // 第二遍"学到了别的形状"（换了骨架、槽位只剩一个）
    let leaner = Write {
        name: "krea2 编辑".into(),
        task: "edit".into(),
        skeleton_hash: "ffff000011112222".into(),
        recipe_version: VERSION,
        source_kind: "file".into(),
        source_ref: Some("file:abc".into()),
        slots_json: r#"[{"role":"loras","nodeId":"71","classType":"LoraLoaderModelOnly","inputName":"lora_name","fileName":"x.safetensors","overrideKey":"71.lora_name","roleGuessed":false}]"#.into(),
        knobs_json: "[]".into(),
        classes_json: r#"["LoraLoaderModelOnly"]"#.into(),
        edges_json: "[]".into(),
        outputs_json: "[]".into(),
        ..Write::default()
    };
    save_row(&conn, &leaner).unwrap();
    let row = get(&conn, id).unwrap().unwrap();
    assert_eq!(row.skeleton_hash, "ffff000011112222");
    assert_eq!(row.task, "edit");
    assert_eq!(
        row.slots.as_array().unwrap().len(),
        1,
        "旧的四个槽位必须被覆盖掉，不许残留"
    );
    assert_eq!(row.slots[0]["overrideKey"], "71.lora_name");
}

#[test]
fn 删掉提示词之后配方还在_只是_prompt_id_变空() {
    let conn = conn();
    let prompt_id = {
        conn.execute(
            "INSERT INTO prompts (title, positive_prompt, source) VALUES ('工作流','x','ComfyUI-File')",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    let id = save_row(
        &conn,
        &sample_write("配方", Some("file:abc"), Some(prompt_id), true, &[]),
    )
    .unwrap()
    .0;
    assert_eq!(get(&conn, id).unwrap().unwrap().prompt_id, Some(prompt_id));

    conn.execute("DELETE FROM prompts WHERE id = ?1", params![prompt_id])
        .unwrap();
    let row = get(&conn, id).unwrap().expect("删提示词不该把配方一起删掉");
    assert_eq!(
        row.prompt_id, None,
        "外键 ON DELETE SET NULL（R1：每条连接都要开 foreign_keys）"
    );
}

#[test]
fn 配方不进同步账本() {
    let conn = conn();
    let id = save_row(
        &conn,
        &sample_write("k1", Some("file:abc"), None, true, &[]),
    )
    .unwrap()
    .0;
    save_row(
        &conn,
        &sample_write("又学了一遍", Some("file:abc"), None, true, &[]),
    )
    .unwrap();
    delete(&conn, id).unwrap();
    let logged: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'workflow_recipes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        logged, 0,
        "这张表刻意不同步（总纲 §3.3）；要同步必须连触发器与 sync.rs 一起改"
    );
    // 白名单本身也钉一下：加进去等于把一份手机跑不了的配方推给手机（R11）
    assert!(!crate::mobile::sync::SYNCED.contains(&"workflow_recipes"));
}

#[test]
fn 缺件的配方照样存_但要能按_complete_过滤出来() {
    let conn = conn();
    save_row(
        &conn,
        &sample_write("完整的", Some("file:ok"), None, true, &[]),
    )
    .unwrap();
    save_row(
        &conn,
        &sample_write(
            "缺件的",
            Some("file:gap"),
            None,
            false,
            &["摘掉节点类：Krea2EditModelPatch".to_string()],
        ),
    )
    .unwrap();
    assert_eq!(
        list(&conn, None, None, None, false, MAX_LIST)
            .unwrap()
            .len(),
        2,
        "默认要能看见缺件的（只是带着标记）"
    );
    let complete: Vec<String> = list(&conn, None, None, None, true, MAX_LIST)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(complete, vec!["完整的"]);
    let gap = list(&conn, None, None, None, false, MAX_LIST)
        .unwrap()
        .into_iter()
        .find(|r| r.name == "缺件的")
        .unwrap();
    assert!(!gap.complete);
    assert_eq!(
        gap.gaps,
        vec!["摘掉节点类：Krea2EditModelPatch".to_string()]
    );
}

#[test]
fn 列表能按_task_与骨架过滤_关键字能搜到模型文件名() {
    let conn = conn();
    let mut edit = sample_write("编辑流", Some("file:e"), None, true, &[]);
    edit.task = "edit".into();
    edit.slots_json = r#"[{"role":"loras","nodeId":"71","classType":"LoraLoaderModelOnly","inputName":"lora_name","fileName":"krea2_identity_edit.safetensors","overrideKey":"71.lora_name","roleGuessed":false}]"#.into();
    save_row(&conn, &edit).unwrap();
    // 换一个骨架指纹：这一条是"另一族"（同族 = 同一个 skeleton_hash）
    let mut t2i = sample_write("文生图", Some("file:t"), None, true, &[]);
    t2i.task = "t2i".into();
    t2i.skeleton_hash = "0000111122223333".into();
    save_row(&conn, &t2i).unwrap();

    let edit_only: Vec<String> = list(&conn, None, Some("edit"), None, false, MAX_LIST)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(edit_only, vec!["编辑流"]);
    let skeleton = list(&conn, None, Some("edit"), None, false, MAX_LIST).unwrap()[0]
        .skeleton_hash
        .clone();
    assert_eq!(
        list(&conn, None, None, Some(&skeleton), false, MAX_LIST)
            .unwrap()
            .len(),
        1
    );
    // 关键字的匹配面要盖住槽位 JSON（"这个模型在哪条配方里"是模型最常问的一件事）
    let by_model: Vec<String> = list(&conn, Some("identity_edit"), None, None, false, MAX_LIST)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(by_model, vec!["编辑流"]);
    assert_eq!(count_by_skeleton(&conn, &skeleton).unwrap(), 1);
}

#[test]
fn 详情能从落库的形状还原出可渲染的配方() {
    let conn = conn();
    let id = save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap()
    .0;
    let row = get(&conn, id).unwrap().unwrap();

    let recipe = row.to_recipe();
    assert_eq!(recipe.task, Task::T2i);
    assert_eq!(
        recipe.skeleton_hash.len(),
        16,
        "指纹是 8 字节 = 16 位十六进制"
    );
    assert_eq!(
        recipe
            .slots
            .iter()
            .map(|s| s.role.clone())
            .collect::<Vec<_>>(),
        vec!["checkpoints"]
    );
    assert!(recipe.knobs.iter().any(|k| k.override_key() == "6.text"));
    assert!(recipe.outputs.iter().any(|o| o.class_type == "SaveImage"));

    let text = row.render_text();
    assert!(text.contains("1.ckpt_name = a.safetensors"), "{text}");
    assert!(text.contains("提示词："), "{text}");
    assert_eq!(
        text.strip_prefix("【工作流配方】")
            .unwrap()
            .split('\n')
            .next()
            .unwrap(),
        row.name
    );
}

#[test]
fn 写不进去就报错_不静默截断() {
    let conn = conn();
    let long_name = "x".repeat(MAX_NAME_CHARS + 1);
    let err = save_row(
        &conn,
        &sample_write(&long_name, Some("file:abc"), None, true, &[]),
    )
    .unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("太长"), "{message}");

    // 指向一条不存在的提示词：报人话，不是 500 + SQL 原文
    let err = save_row(
        &conn,
        &sample_write("配方", Some("file:abc"), Some(999_999), true, &[]),
    )
    .unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("不存在"), "{message}");
    assert!(
        list(&conn, None, None, None, false, MAX_LIST)
            .unwrap()
            .is_empty(),
        "报错的行不许半截写进去"
    );

    // task 不许为空（认不出就写 unknown，别编）
    let mut blank_task = sample_write("配方", Some("file:abc"), None, true, &[]);
    blank_task.task = " ".into();
    assert!(save_row(&conn, &blank_task).is_err());
}

// -----------------------------------------------------------------------
//  学习（`WorkflowRecipeLearner.kt`）
// -----------------------------------------------------------------------

#[test]
fn 学一份_inpaint_图能落库并按_hash_幂等() {
    // 仓库不再随包带模板：用合成 API 图写进 scratch，仍然走"从磁盘文件学"这条入口，
    // 覆盖 任务判定 / 三个模型槽位 / sha 来源 / 落库读回 / 幂等 五件事。
    let conn = conn();
    let deps = deps(None);
    let dir = crate::infra::testpaths::scratch("recipe-learn");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("inpaint_api.json");
    std::fs::write(
        &file,
        r#"{
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"a.safetensors"}},
          "2": {"class_type":"CLIPLoader","inputs":{"clip_name":"te.safetensors","type":"krea2"}},
          "3": {"class_type":"VAELoader","inputs":{"vae_name":"v.safetensors"}},
          "4": {"class_type":"LoadImage","inputs":{"image":"bg.png"}},
          "5": {"class_type":"LoadImageMask","inputs":{"image":"m.png","channel":"red"}},
          "6": {"class_type":"VAEEncode","inputs":{"pixels":["4",0],"vae":["3",0]}},
          "7": {"class_type":"SetLatentNoiseMask","inputs":{"samples":["6",0],"mask":["5",0]}},
          "8": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["2",0]}},
          "9": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["8",0],"negative":["8",0],
                 "latent_image":["7",0],"seed":42,"steps":8,"cfg":1.0,"sampler_name":"euler",
                 "scheduler":"simple","denoise":0.5}},
          "10": {"class_type":"VAEDecode","inputs":{"samples":["9",0],"vae":["3",0]}},
          "11": {"class_type":"SaveImage","inputs":{"images":["10",0]}}
        }"#,
    )
    .unwrap();

    let learned = learn_file(&file, None, false, true, &deps).unwrap();
    assert_eq!(learned.recipe.task, Task::Inpaint);
    assert!(
        learned.recipe.has_mask,
        "有 SetLatentNoiseMask 就该认成带遮罩"
    );
    assert!(learned.complete, "API 格式零转换，不该有缺口");
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(
        learned.source_ref.as_deref(),
        Some(format!("file:{}", sha256_hex(&bytes)).as_str())
    );
    assert_eq!(learned.format, "api");

    let write = write_of(
        &learned.recipe,
        WriteOf {
            name: &learned.name,
            source_kind: &learned.source_kind,
            source_ref: learned.source_ref.as_deref(),
            source_path: learned.source_path.as_deref(),
            prompt_id: learned.prompt_id,
            complete: learned.complete,
            gaps: &learned.gaps,
            extra_warnings: &[],
            notes: None,
        },
    );
    let (id, _) = save_row(&conn, &write).unwrap();
    let row = get(&conn, id).unwrap().unwrap();
    assert_eq!(row.task, "inpaint");
    assert_eq!(learned.recipe.skeleton_hash, row.skeleton_hash);
    assert_eq!(
        row.slots.as_array().unwrap().len(),
        3,
        "底模 + 外挂 CLIP + 外挂 VAE"
    );

    // 再学一遍：还是同一行（幂等键是文件 sha）
    let again = learn_file(&file, None, false, true, &deps).unwrap();
    let again_write = write_of(
        &again.recipe,
        WriteOf {
            name: &again.name,
            source_kind: &again.source_kind,
            source_ref: again.source_ref.as_deref(),
            source_path: again.source_path.as_deref(),
            prompt_id: again.prompt_id,
            complete: again.complete,
            gaps: &again.gaps,
            extra_warnings: &[],
            notes: None,
        },
    );
    let (again_id, created) = save_row(&conn, &again_write).unwrap();
    assert_eq!(id, again_id);
    assert!(!created);
    assert_eq!(
        list(&conn, None, None, None, false, MAX_LIST)
            .unwrap()
            .len(),
        1,
        "幂等：同一份文件学两次只有一行"
    );
}

#[test]
fn 三选一缺参数时报人话_认不出任务时如实说_unknown() {
    let deps = deps(None);
    let err = learn(&LearnArgs::default(), &deps).unwrap_err();
    match err {
        RecipeFailure::Ai { code, message } => {
            assert_eq!(code, "INVALID_ARGUMENT");
            assert!(message.contains("要么给 path"), "{message}");
        }
        other => panic!("{other:?}"),
    }

    // graph 不是节点图 → INVALID_ARGUMENT（界面格式要走文件那条路）
    let err = learn(
        &LearnArgs {
            graph: Some(graph_of(r#"{"nodes":[],"links":[]}"#)),
            ..LearnArgs::default()
        },
        &deps,
    )
    .unwrap_err();
    match err {
        RecipeFailure::Ai { code, message } => {
            assert_eq!(code, "INVALID_ARGUMENT");
            assert!(message.contains("节点图"), "{message}");
        }
        other => panic!("{other:?}"),
    }

    // 库里没有 API 图 → NO_API_GRAPH（不是硬塞一个空配方）
    let err = learn(
        &LearnArgs {
            prompt_id: Some(7),
            ..LearnArgs::default()
        },
        &deps,
    )
    .unwrap_err();
    match err {
        RecipeFailure::Ai { code, message } => {
            assert_eq!(code, "NO_API_GRAPH");
            assert!(message.contains("id=7"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 直接给图_幂等键是图哈希() {
    let deps = deps(None);
    let g = t2i_graph("a.safetensors");
    let a = learn_graph(&g, None, &deps).unwrap();
    let b = learn_graph(&g, None, &deps).unwrap();
    assert_eq!(a.source_kind, "graph");
    assert_eq!(a.name, "节点图", "没给名字时叫「节点图」");
    assert_eq!(a.source_ref, b.source_ref, "同一张图 = 同一个幂等键");
    assert!(a.source_ref.as_deref().unwrap().starts_with("graph:"));
    // 换一个模型文件名：**骨架相同**（泛用的定义），但幂等键不同（图内容变了）
    let c = learn_graph(&t2i_graph("b.safetensors"), Some("换了个底模"), &deps).unwrap();
    assert_eq!(a.recipe.skeleton_hash, c.recipe.skeleton_hash);
    assert_ne!(a.source_ref, c.source_ref);
    assert_eq!(c.name, "换了个底模");
}

// -----------------------------------------------------------------------
//  绑定（`WorkflowRecipeBinderTest.kt`）
// -----------------------------------------------------------------------

/// 一条"分离式加载 + 两条 LoRA"的参考图编辑骨架（形状照 Krea 2 那类工作流抄）。
fn edit_recipe() -> Recipe {
    analyze(&graph_of(
        r#"{
          "55": {"class_type":"UNETLoader","inputs":{"unet_name":"krea2_turbo_bf16.safetensors","weight_dtype":"default"}},
          "56": {"class_type":"CLIPLoader","inputs":{"clip_name":"qwen3vl_4b.safetensors","type":"krea2"}},
          "57": {"class_type":"VAELoader","inputs":{"vae_name":"wan_2.1_vae.safetensors"}},
          "71": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["55",0],"lora_name":"identity_edit.safetensors","strength_model":1.0}},
          "72": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["71",0],"lora_name":"detail.safetensors","strength_model":0.6}},
          "80": {"class_type":"LoadImage","inputs":{"image":"ref.png"}},
          "81": {"class_type":"CLIPTextEncode","inputs":{"clip":["56",0],"text":"换成跑步"}},
          "82": {"class_type":"KSampler","inputs":{"model":["72",0],"positive":["81",0],"latent_image":["80",0],
                 "seed":1,"steps":8,"cfg":1.0,"sampler_name":"euler","scheduler":"simple","denoise":1.0}},
          "83": {"class_type":"VAEDecode","inputs":{"samples":["82",0],"vae":["57",0]}},
          "84": {"class_type":"SaveImage","inputs":{"images":["83",0]}}
        }"#,
    ))
}

fn inventory(models: Vec<LocalModel>) -> Inventory {
    Inventory {
        source: Source::ObjectInfo,
        models,
        models_dir: None,
        truncated: false,
        disk_only: Vec::new(),
        merged: false,
    }
}

fn local(name: &str, folder: &str, visible: bool) -> LocalModel {
    LocalModel {
        name: name.to_string(),
        folder: folder.to_string(),
        path: Some(format!("D:/models/{folder}/{name}")),
        bytes: None,
        visible_to_instance: visible,
    }
}

fn request(file_name: &str) -> Request {
    Request {
        file_name: file_name.to_string(),
        role: None,
        override_key: None,
    }
}

fn bind_with(
    recipe: &Recipe,
    requests: &[Request],
    inv: Option<&Inventory>,
    complete: bool,
    force: bool,
) -> Report {
    bind(
        recipe,
        requests,
        inv,
        Some(7),
        "krea2 编辑",
        complete,
        force,
        &[],
    )
}

#[test]
fn 同名且实例能加载才自动写覆盖键() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "krea2_turbo_v11.safetensors",
        "diffusion_models",
        true,
    )]);
    let report = bind_with(
        &recipe,
        &[request("krea2_turbo_v11.safetensors")],
        Some(&inv),
        true,
        false,
    );
    assert_eq!(report.bindings.len(), 1);
    assert_eq!(report.bindings[0].status, Status::Exact);
    assert_eq!(
        report.overrides,
        vec![(
            "55.unet_name".to_string(),
            "krea2_turbo_v11.safetensors".to_string()
        )]
    );
    assert!(report.ready);
    // 角色是**按清单里那个文件所在的目录推**的（不用调用方猜）
    assert_eq!(report.bindings[0].role.as_deref(), Some("diffusion_models"));
    // 未改动的槽位同样列出（可看出"还有哪些保持原值"）
    assert!(report
        .untouched
        .iter()
        .any(|u| u.starts_with("56.clip_name")));
    // 旋钮提示里有提示词与采样键（模型据此拼下一次提交）
    assert!(report.knob_hints.contains(&"81.text".to_string()));
    assert!(report.knob_hints.contains(&"82.steps".to_string()));
    assert!(
        report.render().contains("可以直接跑"),
        "{}",
        report.render()
    );
    // JSON 形状：applied / status / to 都在，且 overrides 是对象
    let json = report.to_json();
    assert_eq!(json["bindings"][0]["applied"], json!(true));
    assert_eq!(json["bindings"][0]["status"], "exact");
    assert_eq!(
        json["overrides"]["55.unet_name"],
        "krea2_turbo_v11.safetensors"
    );
    assert_eq!(json["recipeId"], json!(7));
}

#[test]
fn 盘上有但实例看不到_不许写键_也不许让人去下载() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "krea2_turbo_bf16.safetensors",
        "diffusion_models",
        false,
    )]);
    let report = bind_with(
        &recipe,
        &[request("krea2_turbo_bf16.safetensors")],
        Some(&inv),
        true,
        false,
    );
    assert_eq!(report.bindings[0].status, Status::OnDiskOnly);
    assert!(
        report.overrides.is_empty(),
        "实例加载不了的文件写进覆盖键 = 下一次提交必然报文件不存在"
    );
    assert!(!report.ready);
    let note = &report.bindings[0].note;
    assert!(note.contains("重启"), "{note}");
    assert!(note.contains("不要下载"), "{note}");
    assert!(
        report.warnings.iter().any(|w| w.contains("不要下载")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn 只有字面相似时给候选但先问用户() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "krea2_turbo_v10.safetensors",
        "diffusion_models",
        true,
    )]);
    let report = bind_with(
        &recipe,
        &[request("krea2_turbo_v11.safetensors")],
        Some(&inv),
        true,
        false,
    );
    let binding = &report.bindings[0];
    assert_eq!(binding.status, Status::Substitute);
    assert!(report.overrides.is_empty());
    assert!(binding.candidates[0].contains("krea2_turbo_v10.safetensors"));
    assert!(binding.note.contains("先问用户"), "{}", binding.note);
}

#[test]
fn 本机没有时绝不编文件名_force_才写得进去() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "anima-base-v1.0.safetensors",
        "diffusion_models",
        true,
    )]);
    let strict = bind_with(
        &recipe,
        &[request("totally_new_model.safetensors")],
        Some(&inv),
        true,
        false,
    );
    assert_eq!(strict.bindings[0].status, Status::Missing);
    assert!(strict.overrides.is_empty());
    assert!(strict.bindings[0].note.contains("不要自己编"));

    let forced = bind_with(
        &recipe,
        &[request("totally_new_model.safetensors")],
        Some(&inv),
        true,
        true,
    );
    assert_eq!(
        forced.overrides,
        vec![(
            "55.unet_name".to_string(),
            "totally_new_model.safetensors".to_string()
        )]
    );
    assert!(
        forced.warnings.iter().any(|w| w.contains("没验证过")),
        "{:?}",
        forced.warnings
    );
    assert!(
        !forced.ready,
        "force 写进去不等于 ready（提交前预检还会再拦一次）"
    );
}

#[test]
fn 拿不到清单时如实说没验证过() {
    let recipe = edit_recipe();
    let unchecked = bind_with(
        &recipe,
        &[Request {
            file_name: "x.safetensors".into(),
            role: Some("diffusion_models".into()),
            override_key: None,
        }],
        None,
        true,
        false,
    );
    assert_eq!(unchecked.bindings[0].status, Status::Unchecked);
    assert!(unchecked.overrides.is_empty());

    let forced = bind_with(
        &recipe,
        &[Request {
            file_name: "x.safetensors".into(),
            role: Some("diffusion_models".into()),
            override_key: None,
        }],
        None,
        true,
        true,
    );
    assert_eq!(
        forced.overrides,
        vec![("55.unet_name".to_string(), "x.safetensors".to_string())]
    );
}

#[test]
fn 同角色多槽位要报_ambiguous_不许一个文件写给两个槽位() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local("new_lora.safetensors", "loras", true)]);
    let report = bind_with(
        &recipe,
        &[Request {
            file_name: "new_lora.safetensors".into(),
            role: Some("loras".into()),
            override_key: None,
        }],
        Some(&inv),
        true,
        false,
    );
    let binding = &report.bindings[0];
    assert_eq!(binding.status, Status::Ambiguous);
    assert!(report.overrides.is_empty());
    assert!(
        binding.note.contains("71.lora_name") && binding.note.contains("72.lora_name"),
        "{}",
        binding.note
    );

    // 点明 overrideKey 就绑得上
    let one = bind_with(
        &recipe,
        &[Request {
            file_name: "new_lora.safetensors".into(),
            role: None,
            override_key: Some("71.lora_name".into()),
        }],
        Some(&inv),
        true,
        false,
    );
    assert_eq!(
        one.overrides,
        vec![(
            "71.lora_name".to_string(),
            "new_lora.safetensors".to_string()
        )]
    );
}

#[test]
fn 整包_checkpoint_接分离式骨架要明说结构上接不上() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "anima-base-v1.0.safetensors",
        "checkpoints",
        true,
    )]);
    let report = bind_with(
        &recipe,
        &[request("anima-base-v1.0.safetensors")],
        Some(&inv),
        true,
        false,
    );
    let binding = &report.bindings[0];
    assert_eq!(binding.status, Status::NotInRecipe);
    assert!(report.overrides.is_empty());
    assert!(binding.note.contains("分离式加载"), "{}", binding.note);
    assert!(binding.note.contains("checkpoint"), "{}", binding.note);
    assert!(binding.note.contains("改写"), "{}", binding.note);
    // 同样的结论也要能从"给了一个不存在的 role"那条路得到
    let by_role = bind_with(
        &recipe,
        &[Request {
            file_name: "x.safetensors".into(),
            role: Some("checkpoints".into()),
            override_key: None,
        }],
        None,
        true,
        false,
    );
    assert!(
        by_role.bindings[0].note.contains("没有 role=checkpoints"),
        "{}",
        by_role.bindings[0].note
    );
}

#[test]
fn 一次绑好几个角色_缺件配方要提醒先补齐() {
    let recipe = edit_recipe();
    let inv = inventory(vec![
        local("new_unet.safetensors", "diffusion_models", true),
        local("new_clip.safetensors", "text_encoders", true),
        local("new_vae.safetensors", "vae", true),
    ]);
    let report = bind_with(
        &recipe,
        &[
            Request {
                file_name: "new_unet.safetensors".into(),
                role: Some("diffusion_models".into()),
                override_key: None,
            },
            Request {
                file_name: "new_clip.safetensors".into(),
                role: Some("text_encoders".into()),
                override_key: None,
            },
            Request {
                file_name: "new_vae.safetensors".into(),
                role: Some("vae".into()),
                override_key: None,
            },
        ],
        Some(&inv),
        false,
        false,
    );
    assert_eq!(
        report.overrides,
        vec![
            (
                "55.unet_name".to_string(),
                "new_unet.safetensors".to_string()
            ),
            (
                "56.clip_name".to_string(),
                "new_clip.safetensors".to_string()
            ),
            ("57.vae_name".to_string(), "new_vae.safetensors".to_string()),
        ]
    );
    assert!(!report.ready, "complete=false 的配方不许报 ready");
    assert!(
        report.hints.iter().any(|h| h.contains("complete=false")),
        "{:?}",
        report.hints
    );
}

#[test]
fn sidecar_元数据不算模型_也不许混进候选() {
    // 真机探针抓到的形状：磁盘兜底清单里有 anima-base-v1.0.metadata.json
    let recipe = edit_recipe();
    let inv = inventory(vec![
        local("anima-base-v1.0.metadata.json", "diffusion_models", true),
        local("anima-base-v1.0.safetensors", "diffusion_models", true),
    ]);
    let report = bind_with(
        &recipe,
        &[Request {
            file_name: "anima-base-v1.0.metadata.json".into(),
            role: Some("diffusion_models".into()),
            override_key: None,
        }],
        Some(&inv),
        true,
        false,
    );
    let binding = &report.bindings[0];
    assert_eq!(binding.status, Status::NotAModel);
    assert!(report.overrides.is_empty());
    assert!(binding.note.contains("safetensors"), "{}", binding.note);

    // 反过来：sidecar 也不该出现在候选里（它会把真模型顶掉）
    let near = bind_with(
        &recipe,
        &[Request {
            file_name: "anima-base-v1.1.safetensors".into(),
            role: Some("diffusion_models".into()),
            override_key: None,
        }],
        Some(&inventory(vec![local(
            "anima-base-v1.0.metadata.json",
            "diffusion_models",
            true,
        )])),
        true,
        false,
    );
    assert!(
        !near.bindings[0]
            .candidates
            .iter()
            .any(|c| c.contains("metadata.json")),
        "候选里不许有 sidecar：{:?}",
        near.bindings[0].candidates
    );
}

#[test]
fn 没给_role_时按底模那一格兜底_但要把假设说出来() {
    let recipe = edit_recipe();
    let report = bind_with(&recipe, &[request("x.safetensors")], None, true, false);
    let binding = &report.bindings[0];
    assert_eq!(
        binding.role.as_deref(),
        Some("diffusion_models"),
        "参数名叫 model，兜底就该落在底模那一格"
    );
    assert_eq!(binding.status, Status::Unchecked);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("按**底模**那一格")),
        "兜底是个假设，必须说出来：{:?}",
        report.warnings
    );
}

#[test]
fn 配方连底模槽位都没有时要说清楚要带什么() {
    // 只有一条 LoRA + 存图的骨架（把底模槽位摘掉）
    let lora_only = analyze(&graph_of(
        r#"{
          "71": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["70",0],"lora_name":"a.safetensors","strength_model":1.0}},
          "70": {"class_type":"UNETLoader","inputs":{"unet_name":""}},
          "84": {"class_type":"SaveImage","inputs":{"images":["70",0]}}
        }"#,
    ));
    let only_loras = Recipe {
        slots: lora_only
            .slots
            .iter()
            .filter(|s| s.role == "loras")
            .cloned()
            .collect(),
        ..lora_only
    };
    let report = bind_with(&only_loras, &[request("x.safetensors")], None, true, false);
    let binding = &report.bindings[0];
    assert_eq!(binding.status, Status::NotInRecipe);
    assert!(binding.note.contains("overrideKey"), "{}", binding.note);
    assert!(
        binding.note.contains("71.lora_name"),
        "要把槽位清单列出来：{}",
        binding.note
    );
    assert!(binding.override_key.is_none());
}

// -----------------------------------------------------------------------
//  已验证绑定（T2，2026-11-09）
// -----------------------------------------------------------------------

/// 一条验证记录（测试中手工构造，避免每次都要发起真实提交）。
fn verified(model: &str, key: &str, run_key: &str) -> Verified {
    Verified {
        model: model.to_string(),
        role: "diffusion_models".to_string(),
        override_key: key.to_string(),
        run_key: run_key.to_string(),
        at: Some("2026-11-09T02:00:00Z".to_string()),
    }
}

/// 回写：同槽位 + 同模型只刷新凭据（不堆行），换模型才是新的一条；读回来能渲染。
#[test]
fn 已验证回写按槽位与模型去重_且不会抹掉别的组合() {
    let conn = conn();
    let id = save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap()
    .0;
    assert!(verified_in(&conn, id).unwrap().is_empty(), "新配方没有验证记录");

    // 第一次跑通
    let written = record_verified(
        &conn,
        id,
        &[verified("krea2_turbo_v11.safetensors", "1.ckpt_name", "run-a")],
    )
    .unwrap();
    assert_eq!(written, 1);
    let list = verified_in(&conn, id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].run_key, "run-a");

    // 同一个组合又跑通一次：**只刷新 runKey / 时间**，不新增
    record_verified(
        &conn,
        id,
        &[verified("krea2_turbo_v11.safetensors", "1.ckpt_name", "run-b")],
    )
    .unwrap();
    let list = verified_in(&conn, id).unwrap();
    assert_eq!(list.len(), 1, "同一个槽位配同一个模型只该有一条");
    assert_eq!(list[0].run_key, "run-b", "要刷新成最近那次");

    // 换了个模型：新的一条，排在最前；旧的那条**一个字都不动**
    record_verified(
        &conn,
        id,
        &[verified("krea2_turbo_v20.safetensors", "1.ckpt_name", "run-c")],
    )
    .unwrap();
    let list = verified_in(&conn, id).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].model, "krea2_turbo_v20.safetensors");
    assert_eq!(list[1].run_key, "run-b");

    // 落库的形状与 DTO 读出来的形状是同一份（列表 / 绑定 / 端点都从这里取）
    let row = get(&conn, id).unwrap().unwrap();
    assert_eq!(row.verified.as_array().unwrap().len(), 2);
    assert_eq!(row.verified[0]["overrideKey"], "1.ckpt_name");
    assert_eq!(row.verified[0]["role"], "diffusion_models");
    let summary = verified_summary(&verified_of(&row.verified)).unwrap();
    assert!(summary.contains("1.ckpt_name=krea2_turbo_v20.safetensors"), "{summary}");
}

/// 重新学一份（`save_row` 走 UPDATE）**不许清掉**已验证记录 ——
/// 那是"这族骨架跑通过"的事实，与这次归纳出来的槽位无关。
#[test]
fn 重新学一份不会清掉已验证记录() {
    let conn = conn();
    let id = save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap()
    .0;
    record_verified(&conn, id, &[verified("a.safetensors", "1.ckpt_name", "r1")]).unwrap();
    save_row(
        &conn,
        &sample_write("krea2 编辑", Some("file:abc"), None, true, &[]),
    )
    .unwrap();
    assert_eq!(verified_in(&conn, id).unwrap().len(), 1);
}

/// 绑定输出里标注"已验证跑通"，并且**不改**能不能自动写覆盖键的判据（仍然只有 exact）。
#[test]
fn 已验证的组合在绑定结果里标出来_但不放松写键的判据() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "krea2_turbo_v11.safetensors",
        "diffusion_models",
        true,
    )]);
    let proven = [verified("krea2_turbo_v11.safetensors", "55.unet_name", "r1")];
    let report = bind(
        &recipe,
        &[request("krea2_turbo_v11.safetensors")],
        Some(&inv),
        Some(7),
        "krea2 编辑",
        true,
        false,
        &proven,
    );
    assert!(report.bindings[0].verified);
    assert_eq!(
        report.bindings[0].verified_at.as_deref(),
        Some("2026-11-09T02:00:00Z")
    );
    assert!(report.render().contains("✔ 已验证跑通"), "{}", report.render());
    assert!(
        report.render().contains("已经跑通过"),
        "要有「直接开跑」的结论：{}",
        report.render()
    );
    let json = report.to_json();
    assert_eq!(json["bindings"][0]["verified"], json!(true));
    assert_eq!(json["bindings"][0]["verifiedAt"], "2026-11-09T02:00:00Z");

    // 没有记录的组合：同一套槽位、没验证过的文件 —— 判据不变，但文案要说"第一跑"
    let report = bind_with(
        &recipe,
        &[request("krea2_turbo_v11.safetensors")],
        Some(&inv),
        true,
        false,
    );
    assert!(!report.bindings[0].verified);
    assert!(report.render().contains("还没跑通过"), "{}", report.render());
}

/// 别人给的文件名写法不同（大小写 / 分隔符）时，仍要认出"这就是上次跑通的那个"。
#[test]
fn 已验证记录按词根集合比模型名_不因写法不同而失效() {
    let recipe = edit_recipe();
    let inv = inventory(vec![local(
        "krea2-turbo-v11.safetensors",
        "diffusion_models",
        true,
    )]);
    let proven = [verified("krea2_turbo_v11.safetensors", "55.unet_name", "r1")];
    let report = bind(
        &recipe,
        &[request("krea2-turbo-v11.safetensors")],
        Some(&inv),
        Some(7),
        "krea2 编辑",
        true,
        false,
        &proven,
    );
    assert!(
        report.bindings[0].verified,
        "同一个文件的另一种写法不该被当成没跑过"
    );
}

