//! `workflow_audit` 的单元测试（主体代码在 [`workflow_audit`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::comfy::comfy_models::{LocalModel, Source};

fn graph(text: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(text)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn find(report: &Report, check: &str) -> Vec<Finding> {
    report
        .findings
        .iter()
        .filter(|f| f.check == check)
        .cloned()
        .collect()
}

/// 一份干净的最小骨架（Krea2 Turbo 的官方档位）。
fn clean_graph() -> Map<String, Value> {
    graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}},
          "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3VL4BAbliterated.safetensors", "type": "krea2"}},
          "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "A candid night photograph of a young woman leaning on a railing, warm promenade lights reflecting on the water, medium shot, vertical framing, soft directional lighting"}},
          "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
          "5": {"class_type": "KSampler", "inputs": {"steps": 8, "cfg": 1.0, "sampler_name": "euler", "seed": 20260922}},
          "6": {"class_type": "VAEDecode", "inputs": {}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "Krea2_run_a"}}
        }"#,
    )
}

#[test]
fn 官方档位的骨架不报必跑不了的问题() {
    let report = audit(&clean_graph(), None, None);
    assert!(
        report.blocking().is_empty(),
        "这份骨架是官方档位、无前端节点：不该有 BLOCK：{}",
        report.render()
    );
    assert!(report.blocking_refusal().is_none());
    // 唯一会报的是"固定 seed + 固定前缀"那条提醒 —— 重复跑同一份图会落同一个文件名，
    // 被去重吞掉（该次无效运行 78 秒）。它是提醒，不是拦截。
    assert_eq!(
        report.findings.iter().map(|f| f.check).collect::<Vec<_>>(),
        vec![CHECK_OUTPUT_NAME],
        "{}",
        report.render()
    );
}

#[test]
fn 纯前端节点必须拦_那次是_20_个() {
    let g = graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}},
          "10": {"class_type": "Label (rgthree)", "inputs": {}},
          "15": {"class_type": "Fast Groups Bypasser (rgthree)", "inputs": {}},
          "163": {"class_type": "Bookmark (rgthree)", "inputs": {}},
          "20": {"class_type": "GetNode", "inputs": {}}
        }"#,
    );
    let report = audit(&g, None, None);
    let f = find(&report, CHECK_FRONTEND);
    assert_eq!(f.len(), 1);
    let f = &f[0];
    assert_eq!(f.level, Level::Block);
    assert!(
        f.detail.contains("0.01"),
        "要说清后果（0.01 秒空跑）：{}",
        f.detail
    );
    for id in ["10", "15", "20", "163"] {
        assert!(f.nodes.contains(&id.to_string()), "{:?}", f.nodes);
    }
    // 拒因里要**点出节点 id**：只说"图里还有 1 个纯前端节点"时，模型只能把整份图重读一遍
    // 去猜是哪一个（2026-10-25 现场为此多花了 3 轮工具）。
    let refusal = report.blocking_refusal().expect("有 BLOCK 就必须给拒因");
    for id in ["10", "15", "20", "163"] {
        assert!(refusal.contains(id), "拒因里没有节点 {id}：\n{refusal}");
    }
}

/// **`/object_info` 里真的有的显示节点不算"纯前端"**（2026-10-25 现场）。
///
/// `krea2 r2i workflow.json` 里的 `55 PreviewAny` 是 ComfyUI **自带**的真节点
/// （`output_node: true`），只是名字命中 `^Preview[A-Z]` 那条模式；
/// 早先它被判成纯前端 → 提交闸门 `MODEL_NOT_LOADABLE` 拒掉整份图，
/// 而 ComfyUI 明明能跑 —— 模型只能反复重读 / 重写工作流，永远过不去。
#[test]
fn object_info_里有的显示节点不算纯前端() {
    let g = graph(
        r#"{
          "1": {"class_type": "VAEDecode", "inputs": {"samples": ["2", 0]}},
          "2": {"class_type": "KSampler", "inputs": {"steps": 8, "cfg": 1.0, "sampler_name": "er_sde", "seed": 1}},
          "3": {"class_type": "PreviewAny", "inputs": {"source": ["1", 0]}},
          "4": {"class_type": "SaveImage", "inputs": {"images": ["1", 0], "filename_prefix": "x"}}
        }"#,
    );
    // ① 拿不到 `/object_info`：静态判据照旧（不知道 ≠ 没问题）
    assert_eq!(find(&audit(&g, None, None), CHECK_FRONTEND).len(), 1);
    // ② 实例里真有这个类：不许再说"服务端根本没有这个类"
    let info: Map<String, Value> = graph(
        r#"{
          "VAEDecode": {"input": {"required": {}}, "output": ["IMAGE"]},
          "KSampler": {"input": {"required": {"steps": ["INT"]}}, "output": ["LATENT"]},
          "PreviewAny": {"input": {"required": {}}, "output": [], "output_node": true},
          "SaveImage": {"input": {"required": {}}, "output": []}
        }"#,
    );
    let findings = find(&audit(&g, Some(&info), None), CHECK_FRONTEND);
    assert!(
        findings.is_empty(),
        "PreviewAny 是 ComfyUI 自带真节点：{}",
        findings[0].detail
    );
    assert!(audit(&g, Some(&info), None).blocking_refusal().is_none());
}

/// 广播型是**零输出桩**：哪怕它注册在 `/object_info` 里，也照样跑不出东西 —— 仍然要拦。
#[test]
fn 广播桩即使注册在_object_info_里也照样算前端类() {
    let g = graph(
        r#"{
          "1": {"class_type": "Anything Everywhere", "inputs": {}},
          "2": {"class_type": "SaveImage", "inputs": {"images": ["1", 0], "filename_prefix": "x"}}
        }"#,
    );
    let info: Map<String, Value> = graph(
        r#"{
          "Anything Everywhere": {"input": {"required": {}}, "output": []},
          "SaveImage": {"input": {"required": {}}, "output": []}
        }"#,
    );
    assert_eq!(find(&audit(&g, Some(&info), None), CHECK_FRONTEND).len(), 1);
}

#[test]
fn 版本不兼容的节点必须拦_并指向更新节点包() {
    let g = graph(
        r#"{
          "94": {"class_type": "WidgetToString", "inputs": {}},
          "74": {"class_type": "SystemNotification|pysssss", "inputs": {}}
        }"#,
    );
    let findings = find(&audit(&g, None, None), CHECK_NODE_VERSION);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.level, Level::Block);
    let fix = f.fix.as_ref().expect("要给出路");
    assert!(
        fix.contains("更新"),
        "要给出路：不是「这工作流坏了」而是「更新节点包」：{fix}"
    );
    assert!(f.detail.contains("IS_CHANGED"), "{}", f.detail);
}

#[test]
fn 固定_seed_加固定前缀必须提醒_那次白跑_78_秒() {
    let g = graph(
        r#"{
          "5": {"class_type": "KSampler", "inputs": {"steps": 8, "cfg": 1.0, "sampler_name": "euler", "seed": 20260921}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "Krea2_spine"}}
        }"#,
    );
    let findings = find(&audit(&g, None, None), CHECK_OUTPUT_NAME);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.level, Level::Warn, "不是「跑不了」，不许拦提交");
    assert!(
        f.detail.contains("未重复入库") || f.detail.contains("mediaCount"),
        "{}",
        f.detail
    );
    let fix = f.fix.as_ref().expect("要给可直接用的修法");
    assert!(fix.contains("overrides"), "要给可直接用的修法：{fix}");
    assert!(
        audit(&g, None, None).blocking_refusal().is_none(),
        "只提醒不拦"
    );
}

#[test]
fn seed_是随机占位符时不提醒() {
    let g = graph(
        r#"{
          "5": {"class_type": "KSampler", "inputs": {"seed": "randomize", "steps": 8}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "x"}}
        }"#,
    );
    assert!(find(&audit(&g, None, None), CHECK_OUTPUT_NAME).is_empty());
}

#[test]
fn 参数偏离该模型的官方档位要提醒() {
    let g = graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}},
          "5": {"class_type": "KSampler", "inputs": {"steps": 30, "cfg": 4.0, "sampler_name": "dpmpp_2m_sde"}}
        }"#,
    );
    let findings = find(&audit(&g, None, None), CHECK_PARAMS);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.level, Level::Warn);
    assert!(f.detail.contains("steps = 30"), "{}", f.detail);
    assert!(f.detail.contains("8"), "要说清该是什么：{}", f.detail);
}

#[test]
fn 不认识的模型不判参数() {
    let g = graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "someUnknownModel.safetensors"}},
          "5": {"class_type": "KSampler", "inputs": {"steps": 999, "cfg": 42.0, "sampler_name": "weird"}}
        }"#,
    );
    assert!(
        find(&audit(&g, None, None), CHECK_PARAMS).is_empty(),
        "不知道就别猜"
    );
}

#[test]
fn 大尺寸加放大加多_lora_要提醒显存() {
    let g = graph(
        r#"{
          "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 2048, "height": 2048}},
          "8": {"class_type": "ImageUpscaleWithModel", "inputs": {}},
          "9": {"class_type": "LoraLoader", "inputs": {"lora_name": "a"}},
          "10": {"class_type": "LoraLoader", "inputs": {"lora_name": "b"}},
          "11": {"class_type": "LoraLoader", "inputs": {"lora_name": "c"}}
        }"#,
    );
    let findings = find(&audit(&g, None, None), CHECK_VRAM);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.level, Level::Warn);
    assert!(f.detail.contains("8GB"), "{}", f.detail);
}

#[test]
fn 给_krea_2_堆_tag_要提醒_给_anima_写整段自然语言也要提醒() {
    let krea = graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2Turbo_something.safetensors"}},
          "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "masterpiece, best quality, 1girl, solo, long hair, blue eyes, white dress, school uniform, blush, smile, looking at viewer"}}
        }"#,
    );
    let krea_findings = find(&audit(&krea, None, None), CHECK_PROMPT_STYLE);
    assert_eq!(krea_findings.len(), 1);
    let krea_finding = &krea_findings[0];
    assert_eq!(krea_finding.level, Level::Warn);
    let fix = krea_finding.fix.as_ref().unwrap();
    assert!(fix.contains("krea-2"), "要指向对应的 skill：{fix}");

    // 一段"逗号很少、但很长"的自然语言 —— 那正是给 Anima 写错了的样子
    let long = "A young woman with short dark blue-purple hair leans down over a young man \
                while her right hand rests gently on the top of his head and she looks down at him \
                with a soft warm teasing smile and a light blush across her cheeks and sweat on her forehead";
    let anima = graph(&format!(
        r#"{{
          "1": {{"class_type": "CheckpointLoaderSimple", "inputs": {{"ckpt_name": "anima-base-v1.0.safetensors"}}}},
          "3": {{"class_type": "CLIPTextEncode", "inputs": {{"text": "{long}"}}}}
        }}"#
    ));
    let anima_findings = find(&audit(&anima, None, None), CHECK_PROMPT_STYLE);
    assert_eq!(anima_findings.len(), 1);
    let anima_finding = &anima_findings[0];
    assert_eq!(
        anima_finding.level,
        Level::Info,
        "Anima 这条是背景提示，不是警告"
    );
    let fix = anima_finding.fix.as_ref().unwrap();
    assert!(fix.contains("anima-prompt"), "{fix}");
}

#[test]
fn 加载器读错目录要提醒_那次是_unet_loader_读_checkpoints() {
    let g = graph(
        r#"{"79": {"class_type": "UNETLoader", "inputs": {"unet_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}}}"#,
    );
    let inv = Inventory {
        source: Source::ObjectInfo,
        models: vec![LocalModel {
            name: "krea2TurboOfficialComfy_krea2TurboInt8.safetensors".to_string(),
            folder: "checkpoints".to_string(),
            path: None,
            bytes: None,
            visible_to_instance: true,
        }],
        models_dir: None,
        truncated: false,
        disk_only: Vec::new(),
        merged: false,
    };
    let findings = find(&audit(&g, None, Some(&inv)), CHECK_LOADER_FOLDER);
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.level, Level::Warn);
    assert!(f.detail.contains("checkpoints"), "{}", f.detail);
    assert!(f.detail.contains("diffusion_models"), "{}", f.detail);
}

#[test]
fn 拿不到_object_info_时不判本机缺节点() {
    let g = graph(r#"{"9": {"class_type": "SomeThirdPartyNode", "inputs": {}}}"#);
    let report = audit(&g, None, None);
    assert!(
        find(&report, CHECK_UNKNOWN_NODE).is_empty(),
        "ComfyUI 没跑（拿不到 /object_info）时不许说「本机没有这个节点类」"
    );
    // 拿得到时才判
    let with_info = audit(&g, Some(&graph(r#"{"KSampler": {}}"#)), None);
    let findings = find(&with_info, CHECK_UNKNOWN_NODE);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].level, Level::Block);
}

#[test]
fn 闸门只拦必跑不了的三项_参数与显存不拦() {
    let g = graph(
        r#"{
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2Turbo_x.safetensors"}},
          "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 2048, "height": 2048}},
          "5": {"class_type": "KSampler", "inputs": {"steps": 40, "cfg": 8.0, "sampler_name": "ddim", "seed": 1}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "same"}}
        }"#,
    );
    let report = audit(&g, None, None);
    assert!(!report.findings.is_empty(), "参数/显存/前缀都该报");
    assert!(
        report.blocking_refusal().is_none(),
        "但没有一项是「跑不了」—— 不许拦：{}",
        report.render()
    );
}

#[test]
fn 非阻断的结论能单独渲染出来_给提交结果的提示用() {
    // 提交闸门放行的那些（参数 / 显存 / 提示词风格 / 输出前缀 / 加载器目录）
    // 要能**单独**渲染成一段提示 —— 拦下的那些走 `blocking_refusal`，不走这里。
    let clean = audit(&clean_graph(), None, None);
    let notes = clean
        .render_non_blocking()
        .expect("固定 seed + 固定前缀那条要单独渲染出来");
    assert!(notes.contains(CHECK_OUTPUT_NAME), "{notes}");
    assert!(!notes.contains("🔴"), "这里只许有非阻断的项：{notes}");

    // 一条结论都没有时**不许**多出一句"审核通过"的噪声（提交结果里那会天天出现）
    let bare = graph(
        r#"{"1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}}}"#,
    );
    assert!(audit(&bare, None, None).render_non_blocking().is_none());
}

#[test]
fn json_形状要能直接被界面与模型用() {
    let g = graph(r#"{"10": {"class_type": "Label (rgthree)", "inputs": {}}}"#);
    let value = audit(&g, None, None).to_json();
    assert_eq!(value["block"], json!(1));
    assert_eq!(value["nodeCount"], json!(1));
    let finding = &value["findings"][0];
    assert_eq!(finding["check"], json!(CHECK_FRONTEND));
    assert!(!finding["fix"].is_null(), "修法要跟着走");
}

/// Kotlin `FrontendNodesTest` 里关于这两条判据的断言（广播 / 显示节点的模式匹配）。
#[test]
fn 广播型与显示型前端节点的判据与_kotlin_一致() {
    for cls in [
        "Anything Everywhere",
        "Anything Everywhere3?",
        "Prompts Everywhere",
        "Seed Everywhere",
    ] {
        assert!(is_broadcast(cls), "{cls} 应当是广播型");
    }
    assert!(is_broadcast("Anything Everywhere (3)"), "模式匹配要认变体");
    assert!(
        is_broadcast("MyEverythingEverywhere"),
        "`Everywhere\\b` 没有前导词边界"
    );
    assert!(
        !is_broadcast("Everywhere3"),
        "`Everywhere\\b` 后面接着词字符时不算（要靠那张表）"
    );
    for cls in [
        "PreviewImage",
        "ShowText",
        "Display Any",
        "DisplayFloat",
        "ShowAnything",
    ] {
        assert!(is_display(cls), "{cls} 应当是显示型");
    }
    assert!(!is_display("KSampler"));
    assert!(!is_broadcast("KSampler"));
    // ⚠ 两张表本身**大小写敏感**（Kotlin 是 `cls in setOf(…)`），变体由模式兜
    assert!(is_broadcast("anything everywhere"), "表不认，但模式认");
    assert!(is_display("previewimage"), "模式是 IGNORE_CASE");
    assert!(is_display("PreviewImage2"), "`^Preview[A-Z]` 认变体");
}

/// `seed` 的数字串（`"20260921"`）也算"写死了" —— 前端把种子存成字符串是常态。
#[test]
fn 种子是数字串时照样算写死() {
    let g = graph(
        r#"{
          "5": {"class_type": "KSampler", "inputs": {"seed": "20260921", "steps": 8}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "x"}}
        }"#,
    );
    let findings = find(&audit(&g, None, None), CHECK_OUTPUT_NAME);
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0].nodes.contains(&"5".to_string()),
        "{:?}",
        findings[0].nodes
    );
    assert!(
        findings[0].nodes.contains(&"7".to_string()),
        "{:?}",
        findings[0].nodes
    );
}

/// `cfg` 的渲染要与 Kotlin 的 `Double.toString()` 一致（`1.0` 不是 `1`）。
#[test]
fn cfg_的渲染与_kotlin_一致() {
    assert_eq!(java_double(1.0), "1.0");
    assert_eq!(java_double(0.0), "0.0");
    assert_eq!(java_double(4.5), "4.5");
}
