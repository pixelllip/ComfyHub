//! `graph_parse` 的单元测试（主体代码在 [`graph_parse`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn graph(text: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(text).unwrap().as_object().unwrap().clone()
}

/// 经典 KSampler 图：steps / cfg / seed / 正负提示词 / 尺寸全都在。
#[test]
fn classic_ksampler_graph() {
    let parsed = parse(Some(&graph(
        r#"{
          "3": {"class_type":"KSampler","inputs":{"seed":12345,"steps":20,"cfg":3.5,
                "sampler_name":"euler","scheduler":"normal","denoise":1.0,
                "model":["4",0],"positive":["6",0],"negative":["7",0],"latent_image":["5",0]}},
          "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":1024,"height":1536,"batch_size":2}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"1girl, cyberpunk","clip":["4",1]}},
          "7": {"class_type":"CLIPTextEncode","inputs":{"text":"lowres, bad hands","clip":["4",1]}},
          "9": {"class_type":"SaveImage","inputs":{"filename_prefix":"krea2/t2i","images":["8",0]}}
        }"#,
    )));
    assert_eq!(parsed.positive, "1girl, cyberpunk");
    assert_eq!(parsed.negative.as_deref(), Some("lowres, bad hands"));
    assert_eq!(parsed.checkpoint.as_deref(), Some("krea2.safetensors"));
    assert_eq!(parsed.steps, Some(20));
    assert_eq!(parsed.cfg, Some(3.5));
    assert_eq!(parsed.seed, Some(12345));
    assert_eq!(parsed.sampler.as_deref(), Some("euler"));
    assert_eq!(parsed.scheduler.as_deref(), Some("normal"));
    assert_eq!((parsed.width, parsed.height, parsed.batch), (Some(1024), Some(1536), Some(2)));
    assert_eq!(parsed.kind, "IMAGE");
    // 标题线索取的是保存节点的文件名前缀（去掉目录）
    assert_eq!(parsed.title_hint.as_deref(), Some("t2i"));
    // extra 里有 denoise，但不该有 seed / steps（那些已经单独入库）
    assert!(parsed.extra.iter().any(|(k, _)| k == "KSampler.denoise"));
    assert!(!parsed.extra.iter().any(|(k, _)| k == "KSampler.seed"));
}

/// LoRA 链：顺着 `model` 往回走，权重取 `strength_model`；同名只留第一次。
#[test]
fn lora_chain_walks_model_links() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,
                "model":["2",0],"positive":["6",0],"latent_image":["5",0]}},
          "2": {"class_type":"LoraLoader","inputs":{"lora_name":"anima\\style\\anima_v2.safetensors",
                "strength_model":0.8,"model":["3",0],"clip":["3",1]}},
          "3": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"ink painting"}}
        }"#,
    )));
    assert_eq!(parsed.loras.len(), 1);
    assert_eq!(parsed.loras[0].name, "anima_v2.safetensors", "只留文件名");
    assert_eq!(parsed.loras[0].weight, 0.8);
    assert_eq!(parsed.checkpoint.as_deref(), Some("krea2.safetensors"));
}

/// rgthree 的 `Lora Loader Stack`：槽位是 `lora_01` + `strength_01`，不是 `lora_name`。
///
/// 这条是真库对账抓出来的：库里 78 份捕获带着这个节点，而提示词的 `loras` 只记到了
/// 旁边的 `Krea2ControlLoRALoader`（或干脆是空的）—— 四个 LoRA 整批静默漏掉。
#[test]
fn rgthree_lora_loader_stack_is_recognized() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,"model":["91",0]}},
          "91": {"class_type":"Lora Loader Stack (rgthree)","inputs":{
                  "model":["6",0],"clip":["4",0],
                  "lora_01":"KNP_000003000.safetensors","strength_01":1.25,
                  "lora_02":"twinbox_late.safetensors","strength_02":0.7,
                  "lora_03":"fajyobore.safetensors","strength_03":0.3,
                  "lora_04":"hiten.safetensors","strength_04":0.3,
                  "lora_05":"None","strength_05":1.0}},
          "6": {"class_type":"UNETLoader","inputs":{"unet_name":"anima-base.safetensors"}},
          "4": {"class_type":"CLIPLoader","inputs":{"clip_name":"qwen_3_06b_base.safetensors"}}
        }"#,
    )));
    assert_eq!(
        parsed.loras.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["KNP_000003000.safetensors", "twinbox_late.safetensors", "fajyobore.safetensors", "hiten.safetensors"],
        "四个槽位都要收，顺序按槽位顺序"
    );
    assert!(
        !parsed.loras.iter().any(|l| l.name == "None"),
        "rgthree 用字符串 None 占位空槽，不能记成名叫 None 的 LoRA"
    );
    assert_eq!(
        parsed.loras.iter().map(|l| l.weight).collect::<Vec<_>>(),
        vec![1.25, 0.7, 0.3, 0.3],
        "权重取同编号的 strength_xx"
    );
    assert_eq!(parsed.checkpoint.as_deref(), Some("anima-base.safetensors"));
}

/// rgthree 的 `Power Lora Loader`：`lora_1` 是嵌套对象，且带 `on` 开关。
///
/// `on: false` 的槽位在 ComfyUI 里没有生效，**不许**记进提示词。
#[test]
fn power_lora_loader_nested_objects_are_recognized() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,"model":["21",0]}},
          "21": {"class_type":"Power Lora Loader (rgthree)","inputs":{
                  "model":["4",0],"clip":["6",0],
                  "lora_1":{"on":true,"lora":"Krea2\\KNPV3_1.safetensors","strength":1},
                  "lora_2":{"on":true,"lora":"Krea2\\sub\\anime style.safetensors","strength":0.6},
                  "lora_3":{"on":false,"lora":"disabled.safetensors","strength":1},
                  "➕ Add Lora":"",
                  "PowerLoraLoaderHeaderWidget":{"type":"PowerLoraLoaderHeaderWidget"}}},
          "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "6": {"class_type":"CLIPLoader","inputs":{"clip_name":"qwen_3_06b_base.safetensors"}}
        }"#,
    )));
    assert_eq!(
        parsed.loras.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["KNPV3_1.safetensors", "anime style.safetensors"],
        "嵌套对象里的名字要收、目录要剥掉、关闭的槽位不要"
    );
    assert_eq!(parsed.loras.iter().map(|l| l.weight).collect::<Vec<_>>(), vec![1.0, 0.6]);
}

/// 栈式加载器（WAS 那类）：名字在 `lora_stack` 指向的**另一个节点**里。
///
/// 只顺着 `model` 链走会漏掉整座栈；顺带钉住两件防误报的事：
/// ① `lora_name_1` 的权重叫 `model_weight_1`；② `lora_strength`（只有强度、没有文件名）
/// 不能被当成一个 LoRA 名字。
#[test]
fn lora_stack_references_and_numbered_slots_are_recognized() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,"model":["10",0]}},
          "10": {"class_type":"Lora Loader (WAS)","inputs":{
                   "model":["50",0],"clip":["6",0],"lora_stack":["30",0]}},
          "30": {"class_type":"Lora Stacker (WAS)","inputs":{
                   "lora_name_1":"a.safetensors","model_weight_1":0.6,
                   "lora_name_2":"b.safetensors","model_weight_2":0.4,
                   "lora_name_3":"","model_weight_3":1}},
          "50": {"class_type":"RapidMOSSAdapter","inputs":{
                   "adapter":"rapid-moss-4step","lora_strength":1.0,"model":["4",0]}},
          "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}},
          "6": {"class_type":"CLIPLoader","inputs":{"clip_name":"qwen_3_06b_base.safetensors"}}
        }"#,
    )));
    assert_eq!(
        parsed.loras.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["a.safetensors", "b.safetensors"],
        "栈里的两个名字都要收；空白槽位与 lora_strength 不许占位"
    );
    assert_eq!(parsed.loras.iter().map(|l| l.weight).collect::<Vec<_>>(), vec![0.6, 0.4]);
}

/// 视频 / 音频类型：`SaveVideo` → VIDEO；纯音频图 → AUDIO；两种都有 → MIXED。
#[test]
fn kind_detection_covers_video_audio_and_mixed() {
    let video = parse(Some(&graph(
        r#"{"1":{"class_type":"SaveVideo","inputs":{"filename_prefix":"v"}},
            "2":{"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}}}"#,
    )));
    assert_eq!(video.kind, "VIDEO");
    let audio = parse(Some(&graph(
        r#"{"1":{"class_type":"SaveAudioMP3","inputs":{}},
            "2":{"class_type":"SaveAudio","inputs":{}}}"#,
    )));
    assert_eq!(audio.kind, "AUDIO");
    let mixed = parse(Some(&graph(
        r#"{"1":{"class_type":"SaveVideo","inputs":{}},"2":{"class_type":"SaveAudio","inputs":{}}}"#,
    )));
    assert_eq!(mixed.kind, "MIXED");
}

/// 自定义采样链（SamplerCustom + Guider + Sigmas + 独立 seed 节点）。
#[test]
fn custom_sampler_chain_reads_sigmas_and_guider() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"SamplerCustomAdvanced","inputs":{"noise":["2",0],"guider":["3",0],
                "sampler":["4",0],"sigmas":["5",0],"latent_image":["6",0]}},
          "2": {"class_type":"RandomNoise","inputs":{"noise_seed":999}},
          "3": {"class_type":"CFGGuider","inputs":{"cfg":2.5,"model":["7",0],"positive":["8",0],"negative":["9",0]}},
          "4": {"class_type":"KSamplerSelect","inputs":{"sampler_name":"dpmpp_2m"}},
          "5": {"class_type":"BasicScheduler","inputs":{"scheduler":"sgm_uniform","steps":6}},
          "6": {"class_type":"EmptySD3LatentImage","inputs":{"width":768,"height":1368}},
          "7": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"shinku.safetensors"}},
          "8": {"class_type":"CLIPTextEncode","inputs":{"text":"maid cafe"}},
          "9": {"class_type":"CLIPTextEncode","inputs":{"text":"blurry"}}
        }"#,
    )));
    assert_eq!(parsed.seed, Some(999), "seed 从 RandomNoise 节点读（采样器自己没有）");
    assert_eq!(parsed.cfg, Some(2.5), "cfg 从 Guider 读");
    assert_eq!(parsed.steps, Some(6), "steps 从 Scheduler 读");
    assert_eq!(parsed.sampler.as_deref(), Some("dpmpp_2m"), "采样器名从 KSamplerSelect 读");
    assert_eq!(parsed.scheduler.as_deref(), Some("sgm_uniform"));
    assert_eq!(parsed.positive, "maid cafe");
    assert_eq!(parsed.negative.as_deref(), Some("blurry"));
    assert_eq!((parsed.width, parsed.height), (Some(768), Some(1368)));
}

/// 负向与正向相同时丢掉（Kotlin `takeIf { it != positive }`）。
#[test]
fn negative_is_dropped_when_it_equals_positive() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1,
                "positive":["2",0],"negative":["3",0],"latent_image":["4",0]}},
          "2": {"class_type":"CLIPTextEncode","inputs":{"text":"same text"}},
          "3": {"class_type":"CLIPTextEncode","inputs":{"text":"same text"}},
          "4": {"class_type":"EmptyLatentImage","inputs":{"width":64,"height":64}}
        }"#,
    )));
    assert_eq!(parsed.positive, "same text");
    assert_eq!(parsed.negative, None);
}

/// **真机形状**（真库 `prompts.id=304` 那份 `capture_runs.raw` 的结构，逐节点抄下来）：
/// Qwen/Anima 那条链 —— `KSampler` 的 `positive`/`negative` 都指向普通 `CLIPTextEncode`，
/// 而 `model` 经过一个 **LoraLoader + SageAttention** 才到 `UNETLoader`。
///
/// 这条用例是**真库对账抓出来的**：第一版在真库上跑 25 条对账，只有这一条的
/// `negative` 是空的（其余 24 条逐字段一致）—— 于是把它固化成用例。
#[test]
fn anima_style_graph_from_the_real_library() {
    let parsed = parse(Some(&graph(
        r#"{
          "1":   {"class_type":"LoraLoader","inputs":{"clip":["72",0],"model":["111",0],
                   "lora_name":"allmm style-step00003000.safetensors","strength_clip":"1","strength_model":"1"}},
          "72":  {"class_type":"CLIPLoader","inputs":{"type":"qwen_image","device":"default","clip_name":"qwen_3_06b_base.safetensors"}},
          "73":  {"class_type":"VAELoader","inputs":{"vae_name":"qwen_image_vae.safetensors"}},
          "74":  {"class_type":"VAEDecode","inputs":{"vae":["73",0],"samples":["77",0]}},
          "75":  {"class_type":"EmptyLatentImage","inputs":{"width":"1024","height":"1024","batch_size":"1"}},
          "76":  {"class_type":"CLIPTextEncode","inputs":{"clip":["113",0],"text":"(worst quality, low quality, normal quality), bad anatomy"}},
          "77":  {"class_type":"KSampler","inputs":{"cfg":"4","seed":"935571105263085","model":["113",0],
                   "steps":"30","denoise":"1","negative":["76",0],"positive":["78",0],
                   "scheduler":"simple","latent_image":["75",0],"sampler_name":"er_sde"}},
          "78":  {"class_type":"CLIPTextEncode","inputs":{"clip":["113",0],"text":"Extreme close-up portrait, anime girl"}},
          "79":  {"class_type":"UNETLoader","inputs":{"unet_name":"anima-base-v1.0.safetensors","weight_dtype":"default"}},
          "107": {"class_type":"SaveImage","inputs":{"images":["74",0],"filename_prefix":"test"}},
          "111": {"class_type":"PathchSageAttentionKJ","inputs":{"model":["79",0],"allow_compile":"False","sage_attention":"auto"}},
          "113": {"class_type":"LoraLoader","inputs":{"clip":["1",0],"model":["1",0],
                   "lora_name":"anima-aesthetic-improvement-v1.1.safetensors","strength_clip":"1","strength_model":"0.8"}}
        }"#,
    )));
    assert_eq!(parsed.positive, "Extreme close-up portrait, anime girl");
    assert_eq!(
        parsed.negative.as_deref(),
        Some("(worst quality, low quality, normal quality), bad anatomy"),
        "负向提示词必须从 sampler.negative 指到的那个 CLIPTextEncode 读出来"
    );
    assert_eq!(parsed.steps, Some(30), "steps 是字符串 \"30\"，要能读");
    assert_eq!(parsed.cfg, Some(4.0));
    assert_eq!(parsed.seed, Some(935571105263085));
    assert_eq!(parsed.sampler.as_deref(), Some("er_sde"));
    assert_eq!(parsed.scheduler.as_deref(), Some("simple"));
    assert_eq!((parsed.width, parsed.height, parsed.batch), (Some(1024), Some(1024), Some(1)));
    // 顺着 model 链收集 LoRA（113 在 1 之前、1 又在 111 之前 —— 文档顺序决定先后）
    assert_eq!(
        parsed.loras.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["anima-aesthetic-improvement-v1.1.safetensors", "allmm style-step00003000.safetensors"]
    );
    assert_eq!(
        parsed.checkpoint.as_deref(),
        Some("anima-base-v1.0.safetensors"),
        "checkpoint 藏在 UNETLoader.unet_name（不是 ckpt_name）"
    );
}

/// **节点引用里的 id 可能是数字**（真机 `"negative": [76, 0]`，而图的键是字符串 `"76"`）。
///
/// 这条是真库对账抓出来的：第一版只认字符串 id，于是负向提示词 / LoRA / checkpoint /
/// 尺寸的**整条引用链静默断掉**（25 条真实记录里，prompt 304 的 negative 解析成空）。
/// 用例同时钉住"数字 id"与"字符串 id"两种写法必须给出**一样**的结果。
#[test]
fn node_references_accept_numeric_ids() {
    let numeric = parse(Some(&graph(
        r#"{
          "76": {"class_type":"CLIPTextEncode","inputs":{"text":"bad hands"}},
          "77": {"class_type":"KSampler","inputs":{"steps":20,"cfg":3,"seed":5,
                 "negative":[76,0],"positive":[78,0],"model":[4,0],"latent_image":[5,0]}},
          "78": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat"}},
          "4":  {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}},
          "5":  {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}}
        }"#,
    )));
    assert_eq!(numeric.negative.as_deref(), Some("bad hands"), "数字 id 的 negative 引用");
    assert_eq!(numeric.positive, "a cat");
    assert_eq!(numeric.checkpoint.as_deref(), Some("m.safetensors"), "数字 id 的 model 引用");
    assert_eq!(numeric.width, Some(512), "数字 id 的 latent_image 引用");

    let strings = parse(Some(&graph(
        r#"{
          "76": {"class_type":"CLIPTextEncode","inputs":{"text":"bad hands"}},
          "77": {"class_type":"KSampler","inputs":{"steps":20,"cfg":3,"seed":5,
                 "negative":["76",0],"positive":["78",0],"model":["4",0],"latent_image":["5",0]}},
          "78": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat"}},
          "4":  {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}},
          "5":  {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}}
        }"#,
    )));
    assert_eq!(numeric, strings, "两种 id 写法必须解析成同一个结果");
}

/// 空图 / null / 脏数据都不许 panic，也不许编字段。
#[test]
fn empty_and_dirty_graphs_are_safe() {
    assert_eq!(parse(None), Parsed::default());
    assert_eq!(parse(Some(&Map::new())), Parsed::default());
    // 值不是对象（脏数据）：整条丢掉
    let dirty = graph(r#"{"1":"nope","2":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1}}}"#);
    let parsed = parse(Some(&dirty));
    assert_eq!(parsed.steps, Some(1));
    // 类型全错的 inputs：不许 panic，字段留空
    let weird = parse(Some(&graph(
        r#"{"1":{"class_type":"KSampler","inputs":{"steps":"abc","cfg":null,"seed":{}}}}"#,
    )));
    assert_eq!(weird.steps, None);
    assert_eq!(weird.cfg, None);
    assert_eq!(weird.seed, None);
}

/// 文本兜底：没有条件链时取最长的文本输入；**并列取第一个**（Kotlin `maxByOrNull`）。
#[test]
fn longest_text_fallback_takes_the_first_on_ties() {
    let parsed = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}},
          "2": {"class_type":"CLIPTextEncode","inputs":{"text":"AAAA"}},
          "3": {"class_type":"CLIPTextEncode","inputs":{"text":"BBBB"}}
        }"#,
    )));
    assert_eq!(parsed.positive, "AAAA", "并列时必须取文档顺序里的第一个");

    let longer = parse(Some(&graph(
        r#"{
          "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}},
          "2": {"class_type":"CLIPTextEncode","inputs":{"text":"AAAA"}},
          "3": {"class_type":"CLIPTextEncode","inputs":{"text":"BBBBBBBB"}}
        }"#,
    )));
    assert_eq!(longer.positive, "BBBBBBBB", "更长的那条才算正向");
}

/// 运行时生成的正向提示词：图里只有空槽，最终文本在 `/history` 的 `outputs` 里。
///
/// 真机形状（用户 2026-10-25 的 `test_00901_.png`）：
/// `KSampler.positive → CLIPTextEncode.text → PreviewAny.source → ComfySwitchNode → TextGenerate`。
#[test]
fn final_positive_reads_the_runtime_output() {
    let graph = graph(
        r#"{
          "128": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,
                  "positive":["108",0],"negative":["133",0]}},
          "108": {"class_type":"CLIPTextEncode","inputs":{"text":["55",0],"clip":["4",1]}},
          "55":  {"class_type":"PreviewAny","inputs":{"source":["162",0]}},
          "162": {"class_type":"ComfySwitchNode","inputs":{"on_true":["43",0],"on_false":["182",0],"switch":true}},
          "43":  {"class_type":"TextGenerate","inputs":{"system_prompt":["45",0],"prompt":["182",0]}},
          "45":  {"class_type":"PrimitiveStringMultiline","inputs":{"value":"You are an expert prompt engineer."}},
          "182": {"class_type":"StringConcatenate","inputs":{"string_a":["147",0],"string_b":["180",0]}},
          "147": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}},
          "180": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}},
          "133": {"class_type":"CLIPTextEncode","inputs":{"text":""}}
        }"#,
    );
    let outputs: Map<String, Value> = serde_json::from_str(
        r#"{"55":{"text":["A medium close-up of a girl with green hair."]},
            "2":{"images":[{"filename":"test_00901_.png"}]}}"#,
    )
    .unwrap();

    // 光看图：正向是空的（这正是画廊里一直空着的原因）
    assert_eq!(parse(Some(&graph)).positive, "");
    // 加上运行时输出：拿到最终那一整段
    assert_eq!(
        final_positive(Some(&graph), Some(&outputs)).as_deref(),
        Some("A medium close-up of a girl with green hair.")
    );
}

/// 字面量正向由 [`parse`] 直接拿到，**不许**被旁边某个预览节点的文本顶掉。
#[test]
fn final_positive_leaves_literal_text_to_parse() {
    let graph = graph(
        r#"{
          "128": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"positive":["108",0]}},
          "108": {"class_type":"CLIPTextEncode","inputs":{"text":"literal prompt"}},
          "55":  {"class_type":"PreviewAny","inputs":{"source":["43",0]}}
        }"#,
    );
    let outputs: Map<String, Value> =
        serde_json::from_str(r#"{"55":{"text":["some other text"]}}"#).unwrap();
    assert_eq!(final_positive(Some(&graph), Some(&outputs)), None);
}

/// 编码器的 `text` 直接连着开关、预览在开关后面 —— 转发节点也要能穿过去。
#[test]
fn final_positive_follows_forwarding_nodes() {
    let graph = graph(
        r#"{
          "128": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"positive":["108",0]}},
          "108": {"class_type":"CLIPTextEncode","inputs":{"text":["162",0]}},
          "162": {"class_type":"ComfySwitchNode","inputs":{"on_true":["55",0],"on_false":["147",0]}},
          "55":  {"class_type":"PreviewAny","inputs":{"source":["162",0]}},
          "147": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}}
        }"#,
    );
    let outputs: Map<String, Value> =
        serde_json::from_str(r#"{"55":{"text":["generated by a local LLM"]}}"#).unwrap();
    assert_eq!(
        final_positive(Some(&graph), Some(&outputs)).as_deref(),
        Some("generated by a local LLM")
    );
}

/// 找不到"喂给正向条件"的证据时**一个字都不编**（返回 None，调用方保留原解析）。
#[test]
fn final_positive_never_guesses_from_unrelated_outputs() {
    let graph = graph(
        r#"{
          "128": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"positive":["108",0]}},
          "108": {"class_type":"CLIPTextEncode","inputs":{"text":["182",0]}},
          "182": {"class_type":"StringConcatenate","inputs":{"string_a":["147",0],"string_b":["180",0]}},
          "147": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}},
          "180": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}}
        }"#,
    );
    let outputs: Map<String, Value> =
        serde_json::from_str(r#"{"999":{"text":["a note that was never used"]}}"#).unwrap();
    assert_eq!(final_positive(Some(&graph), Some(&outputs)), None);
}

/// 大 seed（> i64 范围）**饱和**成 `i64::MAX`（Kotlin 的第二档 `toDoubleOrNull()?.toLong()`）。
#[test]
fn huge_seed_saturates_like_kotlin() {
    let parsed = parse(Some(&graph(
        r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"seed":18446744073709551615}}}"#,
    )));
    assert_eq!(parsed.seed, Some(i64::MAX));
    // 还在 i64 范围内的大数按 i64 直读（不能先过 f64：那会丢精度）
    let normal = parse(Some(&graph(
        r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"seed":9007199254740993}}}"#,
    )));
    assert_eq!(normal.seed, Some(9007199254740993));
}

/// `extra` 的上限与 Kotlin **一模一样**：`>= 60` 就停，但那条 `__node_id` 不经过检查，
/// 所以实际长度**最多 61**（60 来自 `__node_id`、下一条 push 才触发 break）。
/// 别"顺手修正"成严格 60 —— 那会与 Kotlin 差一条，而这一条会进 `extra_params`。
#[test]
fn extra_cap_matches_kotlin_exactly() {
    let mut nodes = String::from("{");
    for index in 0..40 {
        if index > 0 {
            nodes.push(',');
        }
        nodes.push_str(&format!(
            r#""{index}":{{"class_type":"Node{index}","inputs":{{"a":"1","b":"2","c":"3"}}}}"#
        ));
    }
    nodes.push('}');
    let parsed = parse(Some(&graph(&nodes)));
    assert_eq!(parsed.extra.len(), 61, "Kotlin 的 break 在 push 之后 → 最多 61");
    assert_eq!(parsed.extra[0].0, "Node0.a");
    // 少量节点时不会有上限，且每个节点末尾都带一条 `__node_id`
    let small = parse(Some(&graph(
        r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"denoise":0.5}}}"#,
    )));
    assert!(small.extra.iter().any(|(k, v)| k == "KSampler.__node_id" && v == "1"));
    assert!(small.extra.iter().any(|(k, v)| k == "KSampler.denoise" && v == "0.5"));
}
