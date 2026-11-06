//! ComfyUI 侧：查询两件、状态/队列/进度、读工作流、体检/审核/摘拼、图像编辑投放、起停实例。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

/// **投放要跟着实例报出来的那份 input 走**（2026-09-30 那个 `400 Invalid image file` 的回归线）。
///
/// 起实例时挂哪个 input 有两处来源（调用方写的 `--input`、ComfyHub 补的 `--input-directory`），
/// 光看配置推不出来。所以投放工具会**问一次实例**：拿它 `/object_info/LoadImage` 的候选名单
/// 去比对候选目录，覆盖得最多的那个就是它在读的 input。
///
/// 这一条钉的是**那条 HTTP 路径真的接通了**：`LoadImage.input.required.image[0]` 的解析、
/// 目录归属判定、以及结果里的 `inputDirSource`（诊断用 —— 投错目录时先看它）。
#[tokio::test]
async fn 投放要问实例它到底在读哪个_input() {
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);
    let input = root.join("shared").join("input");
    // 实例的名单里放一个**只在配置那份额 input 里才有**的文件 —— 探到它就说明找到了正主。
    // （真机上的判据就是这个：安装目录那份与共享那份有一批文件是不重叠的。）
    std::fs::write(input.join("only-in-shared-00000001.png"), b"x").unwrap();

    let info: &'static str = Box::leak(
        serde_json::json!({
            "LoadImage": { "input": { "required": {
                "image": [["only-in-shared-00000001.png", "example.png"], {}]
            } } }
        })
        .to_string()
        .into_boxed_str(),
    );
    let (base, gateway) = fake_comfy_full(r#"{}"#, b"", info).await;
    // 让这个状态指向假实例（`configure_shared_output` 里写的是连不上的 127.0.0.1:9）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE app_settings SET v = json_set(v, '$.comfyUrl', ?1) WHERE k = 'capture.config'",
                rusqlite::params![base],
            )?;
            Ok(())
        })
        .unwrap();

    let source = root.join("ai-attachments").join("att-probe.png");
    write_test_png(&source, 64, 48);
    seed_attachment(&state, "att-probe", "参考图.png", "image");

    let out = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-probe" }),
    )
    .await
    .unwrap();
    assert_eq!(
        out.payload["inputDir"],
        input.display().to_string(),
        "探到实例读的就是这一份"
    );
    assert_eq!(
        out.payload["inputDirSource"], "instance",
        "来路要如实标出来：{}",
        out.payload
    );
    assert!(
        out.payload.get("inputDirNote").is_none(),
        "两边一致时不该有 note"
    );
    let filename = out.payload["filename"].as_str().unwrap();
    assert!(
        input.join(filename).is_file(),
        "要真的落盘：{}",
        input.join(filename).display()
    );

    // ⚠ **"实例说的那一份与我们推的那一份不一致"** 那半不在这里测：探测只能返回**候选里**的一个，
    //   而候选来自配置 / Desktop 配置 / 盘探 —— 要造出"候选第二名胜出"就得动用户的真实
    //   `%APPDATA%\Comfy Desktop`（测试不许碰）。那半是纯函数
    //   `image_edit::tests::实例说的_input_覆盖我们按配置推的那份` 的活。
    gateway.abort();
}

#[tokio::test]
async fn 投放附件按配置的输出目录推_input_并真的落盘() {
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);
    // 项目内也有一份"看起来像 ComfyUI"的目录：**配置优先**，不许写到这里来
    std::fs::create_dir_all(root.join("comfyui").join("input")).unwrap();
    std::fs::create_dir_all(root.join("comfyui").join("output")).unwrap();

    let source = root.join("ai-attachments").join("att-abc.png");
    write_test_png(&source, 64, 48);
    seed_attachment(&state, "att-abc", "参考 图.png", "image");

    let out = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-abc" }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["attachmentId"], "att-abc");
    let filename = out.payload["filename"].as_str().unwrap().to_string();
    // 名字里带来源 id 里的字母数字（`att-abc` → `attabc`），重复调用得到同一个文件
    assert_eq!(filename, "参考_图-attabc.png");
    assert_eq!(
        out.payload["inputDir"],
        root.join("shared").join("input").display().to_string()
    );
    let landed = root.join("shared").join("input").join(&filename);
    assert!(landed.is_file(), "投放的图没落盘：{}", landed.display());
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        std::fs::read(&source).unwrap(),
        "投放的是另一个文件"
    );
    assert!(
        !root.join("comfyui").join("input").join(&filename).exists(),
        "配置里已经指定了输出目录，却写进了项目内的 comfyui"
    );
    assert!(
        out.payload["hint"].as_str().unwrap().contains(&filename),
        "提示里要带真实文件名"
    );

    // 非图片附件：如实报 NOT_AN_IMAGE（ComfyUI 读不了 mp4）
    seed_attachment(&state, "att-vid", "clip.mp4", "video");
    let not_image = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-vid" }),
    )
    .await
    .unwrap_err();
    assert_eq!(not_image.code, "NOT_AN_IMAGE");
    assert!(
        not_image.message.contains("mp4"),
        "要说清是哪种类型不符：{}",
        not_image.message
    );

    // 库里没有这个 id
    let missing = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-none" }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "ATTACHMENT_NOT_FOUND");

    // 库里有行、盘上没文件：也如实说（别走到拷贝再报一个看不懂的 IO 错）
    // ⚠ 文案是"原件已经不在了"而不是"文件不在"：`file_of` 找不到文件时回 None，
    //   走的是前一条分支（与 Kotlin 同一套措辞）
    seed_attachment(&state, "att-gone", "gone.png", "image");
    let gone = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-gone" }),
    )
    .await
    .unwrap_err();
    assert_eq!(gone.code, "ATTACHMENT_NOT_FOUND");
    assert!(gone.message.contains("原件"), "{}", gone.message);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn 投放画廊产物会带原图尺寸并拒绝非图片() {
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);

    let stored = "real-pic.png";
    write_test_png(&state.media_dir().join(stored), 320, 240);
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes) \
                 VALUES (310, 'IMAGE', '阶段一的人物图', 'pic.png', ?1, 4096)",
                rusqlite::params![stored],
            )?;
            tx.execute(
                "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes) \
                 VALUES (311, 'VIDEO', '一段视频', 'clip.mp4', ?1, 4096)",
                rusqlite::params![stored],
            )?;
            Ok(())
        })
        .unwrap();

    let out = call_tool(
        &state,
        "comfy_use_media",
        serde_json::json!({ "mediaId": 310 }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["mediaId"], 310);
    assert_eq!(
        out.payload["filename"], "pic-m310.png",
        "名字要带 mediaId 的前缀，防止两张同名图互相覆盖"
    );
    assert_eq!(
        out.payload["width"], 320,
        "遮罩要照原图尺寸生成，所以尺寸必须给出来"
    );
    assert_eq!(out.payload["height"], 240);
    let landed = root.join("shared").join("input").join("pic-m310.png");
    assert!(landed.is_file());

    // 字符串形式的 id 也要认（模型经常把 id 写成 "310"）
    let as_text = call_tool(
        &state,
        "comfy_use_media",
        serde_json::json!({ "mediaId": "310" }),
    )
    .await
    .unwrap();
    assert_eq!(as_text.payload["filename"], "pic-m310.png");

    let video = call_tool(
        &state,
        "comfy_use_media",
        serde_json::json!({ "mediaId": 311 }),
    )
    .await
    .unwrap_err();
    assert_eq!(video.code, "NOT_AN_IMAGE");
    assert!(video.message.contains("VIDEO"), "{}", video.message);

    let missing = call_tool(
        &state,
        "comfy_use_media",
        serde_json::json!({ "mediaId": 999 }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "MEDIA_NOT_FOUND");

    let no_arg = call_tool(&state, "comfy_use_media", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(no_arg.code, "INVALID_ARGUMENT");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn 遮罩照底图尺寸画且白区就是重绘区() {
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);
    let input = root.join("shared").join("input");
    write_test_png(&input.join("bg.png"), 200, 100);

    // ① 给了 baseFilename：尺寸照底图，白区在右半边
    let out = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": "bg.png", "region": "0.5,0,0.5,1", "filename": "m.png" }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["width"], 200);
    assert_eq!(out.payload["height"], 100);
    assert_eq!(out.payload["filename"], "m.png");
    assert_eq!(out.payload["whiteMeans"], "要重新生成的地方（重绘区）");
    assert_eq!(out.payload["approximate"], false, "显式给了坐标就不算估的");
    let mask = image::open(input.join("m.png")).unwrap().to_luma8();
    assert_eq!((mask.width(), mask.height()), (200, 100));
    assert_eq!(mask.get_pixel(150, 50)[0], 255, "右半边是重绘区");
    assert_eq!(mask.get_pixel(10, 50)[0], 0, "左半边要原样保留");

    // ② invert：把里外翻过来（"换背景保人物"就是这一档）
    let inverted = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": "bg.png", "region": "0.5,0,0.5,1", "invert": true, "filename": "mi.png" }),
    )
    .await
    .unwrap();
    assert_eq!(inverted.payload["invert"], true);
    let mask = image::open(input.join("mi.png")).unwrap().to_luma8();
    assert_eq!(mask.get_pixel(150, 50)[0], 0);
    assert_eq!(mask.get_pixel(10, 50)[0], 255);

    // ③ 不给 baseFilename 就按显式尺寸；**尺寸不合法要拦**（16~8192）
    let explicit = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "width": 64, "height": 32, "region": "person" }),
    )
    .await
    .unwrap();
    assert_eq!(explicit.payload["width"], 64);
    assert_eq!(
        explicit.payload["approximate"], true,
        "预设 region 是粗框，必须如实说是估的"
    );
    let too_small = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "width": 8, "height": 8, "region": "full" }),
    )
    .await
    .unwrap_err();
    assert_eq!(too_small.code, "INVALID_ARGUMENT");
    let neither = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "region": "full" }),
    )
    .await
    .unwrap_err();
    assert_eq!(neither.code, "INVALID_ARGUMENT");

    // ④ baseFilename 里的路径要被剥掉（ComfyUI 只认裸文件名），剥完找不到就如实报
    let traversal = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": "../../../windows/system32/evil.png" }),
    )
    .await
    .unwrap_err();
    assert_eq!(traversal.code, "INVALID_ARGUMENT");
    assert!(
        traversal.message.contains("evil.png"),
        "{}",
        traversal.message
    );

    // ⑤ 同一组参数重复调用得到**同一个文件**（不在 input 目录里堆垃圾）
    let first = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": "bg.png", "region": "center", "feather": 16 }),
    )
    .await
    .unwrap();
    let again = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": "bg.png", "region": "center", "feather": 16 }),
    )
    .await
    .unwrap();
    assert_eq!(first.payload["filename"], again.payload["filename"]);
    assert!(first.payload["filename"]
        .as_str()
        .unwrap()
        .starts_with("comfyhub_mask_"));

    std::fs::remove_dir_all(&root).ok();
}

/// 贴人物档（`mode="subject"`，2026-09-27 起**唯一**一条贴人路）：**不写遮罩**，
/// 也**一个像素都不改**人物图 —— 抠图与前景估计都由工作流里的模型做（`refine_foreground`）。
///
/// ⛔ 颜色抠图那一档（`matteSource="color"` + despill/反混/choke/ring）已整档删除：
/// 它会在人物轮廓**外**留一圈青绿（剪影外那条带 39.6% 像素偏绿，比底图多 +34.8pp），
/// 而且**发梢被整段吃掉**。所以这里同时钉住"老参数会被明确拒绝"。
#[tokio::test]
async fn 贴人物档不写遮罩也不碰人物图交给工作流去抠() {
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);
    let input = root.join("shared").join("input");
    write_test_png(&input.join("bg.png"), 64, 48);

    // 人物图：绿幕 + 中间一块"人"，**紧贴人外面那两圈是"人 + 绿幕"的混合色**
    //（颜色离背景很近 → 颜色键把它判成背景 → 它落在掩码**外面**，正是当初"只往内压盖不到"的那批）
    let (w, h) = (64u32, 48u32);
    let mut person = image::RgbImage::from_pixel(w, h, image::Rgb([0, 177, 64]));
    for y in 8..40u32 {
        for x in 16..48u32 {
            person.put_pixel(x, y, image::Rgb([200, 40, 60]));
        }
    }
    for y in 6..42u32 {
        for x in 14..50u32 {
            if !(8..40).contains(&y) || !(16..48).contains(&x) {
                person.put_pixel(x, y, image::Rgb([20, 170, 60]));
            }
        }
    }
    person.save(input.join("person.png")).unwrap();
    let person_bytes = std::fs::read(input.join("person.png")).unwrap();

    let out = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({
            "mode": "subject",
            "baseFilename": "bg.png",
            "subjectFilename": "person.png",
            "bgColor": "green"
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        out.payload["maskGenerated"], false,
        "贴人物档不许写合成遮罩"
    );
    assert!(
        out.payload.get("filename").is_none(),
        "没有遮罩文件名可给：{:?}",
        out.payload["filename"]
    );
    assert_eq!(
        out.payload["personFilename"], "person.png",
        "要把人物图文件名回给调用方（它才是人物 LoadImage 该填的）"
    );
    // 定位器诊断还在（它只影响摆放），但要说清"不参与合成"
    assert!(
        out.payload["subjectRatio"].as_f64().unwrap() > 0.005,
        "{:?}",
        out.payload["subjectRatio"]
    );
    assert!(out.payload["locatorNote"]
        .as_str()
        .unwrap()
        .contains("定位器"));
    let hint = out.payload["hint"].as_str().unwrap();
    assert!(
        hint.contains("refine_foreground=true") && hint.contains("apiGraph"),
        "hint 要指向那条抠图链、并要求从 apiGraph 现读 id：{hint}"
    );
    assert!(
        hint.contains("不要") && hint.contains("LoadImageMask"),
        "必须明确说「**不要**覆盖遮罩」：{hint}"
    );
    assert!(
        !hint.contains("12.image"),
        "不该出现「覆盖 12.image」那种指引（那份工作流没有 12 / 26）：{hint}"
    );

    // input 目录里**不许**多出遮罩 / ring（写一张没人用的文件只会在用户目录里攒垃圾）
    let strays: Vec<String> = std::fs::read_dir(&input)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("comfyhub_mask_") || n.starts_with("ring-"))
        .collect();
    assert!(
        strays.is_empty(),
        "贴人物档在 input 目录里留了垃圾：{strays:?}"
    );

    // ★ **一个像素都不许改人物图**：抠图与**前景估计**都在工作流里做（`easy imageRemBg` +
    //   `refine_foreground=true`），用的是**模型自己的 alpha**；Rust 这边只有"按颜色猜的覆盖率"，
    //   试过三次都更差（压通道→青边、两侧反混→白光晕）。用**逐字节相同**钉住"别碰"。
    let after = std::fs::read(input.join("person.png")).unwrap();
    assert_eq!(
        after, person_bytes,
        "贴人物档必须**原样**把人物图交给工作流（抠图与前景估计都由模型做）"
    );

    // ⛔ 已删除那一档的老参数：**明确拒绝**（静默忽略 = 模型以为遮罩是自己写的，去覆盖不存在的节点）
    for removed in comfyhub_server_rs::comfy::image_edit::REMOVED_ARGS {
        let mut map = serde_json::Map::new();
        map.insert((*removed).to_string(), serde_json::json!("model"));
        map.insert("width".to_string(), serde_json::json!(64));
        map.insert("height".to_string(), serde_json::json!(48));
        map.insert("region".to_string(), serde_json::json!("full"));
        let err = call_tool(&state, "comfy_make_mask", serde_json::Value::Object(map))
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{removed} 应当被明确拒绝");
        assert!(
            err.message.contains(removed) && err.message.contains("已删除"),
            "{}",
            err.message
        );
    }

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn 图像编辑三件在真库夹具上串起来能用() {
    // ⚠ "找不到 ComfyUI"那一支**不在这里验**：这台开发机装着 ComfyUI，
    //   `comfy_dirs::find_home` 一定探得到（连 Desktop 安装形态都认），
    //   于是那条分支在集成用例里永远走不到 —— 而它恰恰是新用户第一次用时会撞上的那一支。
    //   它由 `image_edit.rs` 的单测（`找不到_comfyui_时如实报错`）用纯函数直接钉住。
    let state = state();
    let root = state.project_root();
    configure_shared_output(&state, &root);

    // 一次完整的"投放底图 → 生成遮罩"：前一件产出的文件名要能直接喂给下一件
    let source = root.join("ai-attachments").join("att-flow.png");
    write_test_png(&source, 128, 96);
    seed_attachment(&state, "att-flow", "ref.png", "image");
    let placed = call_tool(
        &state,
        "comfy_use_attachment",
        serde_json::json!({ "attachmentId": "att-flow" }),
    )
    .await
    .unwrap();
    let filename = placed.payload["filename"].as_str().unwrap().to_string();

    let mask = call_tool(
        &state,
        "comfy_make_mask",
        serde_json::json!({ "baseFilename": filename, "region": "0.58,0.26,0.36,0.74", "feather": 16 }),
    )
    .await
    .unwrap();
    assert_eq!(
        mask.payload["width"], 128,
        "遮罩尺寸必须与投放进去的底图一致"
    );
    assert_eq!(mask.payload["height"], 96);
    assert_eq!(mask.payload["regionNormalized"], "[0.58,0.26,0.36,0.74]");
    let mask_file = root
        .join("shared")
        .join("input")
        .join(mask.payload["filename"].as_str().unwrap());
    assert!(mask_file.is_file(), "遮罩没落盘：{}", mask_file.display());
    // 提示里要说清下一个节点该覆盖哪个输入（模型就是照这句话写 overrides 的）
    assert!(mask.content.contains("LoadImageMask"), "{}", mask.content);

    std::fs::remove_dir_all(&root).ok();
}

// ===========================================================================
//  comfy 查询两件（P4 第 41 步）：comfy_get_run / comfy_list_workflows
// ===========================================================================

/// `comfy_get_run`：**两个入口都认**（UUID 形式的 run_key 与数字 prompt_id），
/// 且 `capturedAt` 要过 `iso_time`（Kotlin 是"先解析再按 Java Instant.toString() 重排"，
/// 库里 `…:00.000Z` 在那边发出来是**没有小数点**的 —— 原样透传就会两端差一个字符串）。
#[tokio::test]
async fn comfy_get_run_accepts_both_keys_and_reformats_time() {
    let state = state();
    // 夹具里已经有一条 capture_runs（run_key = 'run:abc'，见 test_conn）
    let out = call_tool(
        &state,
        "comfy_get_run",
        serde_json::json!({ "runKey": "run:abc" }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["runKey"], "run:abc");
    assert_eq!(out.payload["status"], "success");
    assert_eq!(out.payload["mediaCount"], 0);
    assert_eq!(
        out.payload["capturedAt"], "2026-09-23T02:00:00Z",
        "时间必须按 Java 的规则重排（.000Z → 不带小数）：{}",
        out.payload
    );
    // 可空键**整个不出现**（Kotlin 的 explicitNulls = false）
    assert!(out.payload.get("title").is_none(), "{}", out.payload);
    assert!(out.payload.get("error").is_none(), "{}", out.payload);
    // 正文就是同一份 JSON（模型读正文、界面读 meta，两边形状一致）
    assert_eq!(out.content, out.payload.to_string());

    // 数字 prompt_id 那个入口也认（模型很自然会把 capturedPromptId 拿来查）。
    // ⚠ `capture_runs.prompt_id` 上有外键 → 只能指向真实存在的提示词（夹具里是 10/11）；
    // 这也顺手是"外键真的在生效"的又一次实据（第一版写成指向 capture_runs 自己的 id，当场被拒）。
    state
        .with(|conn| {
            conn.execute(
                "UPDATE capture_runs SET prompt_id = 10 WHERE run_key = 'run:abc'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let by_number = call_tool(
        &state,
        "comfy_get_run",
        serde_json::json!({ "runKey": "10" }),
    )
    .await
    .unwrap();
    assert_eq!(by_number.payload["runKey"], "run:abc");
    assert_eq!(by_number.payload["promptId"], 10);

    // 查不到：NOT_FOUND + 那句能把人引到正确 id 的人话
    let missing = call_tool(
        &state,
        "comfy_get_run",
        serde_json::json!({ "runKey": "nope" }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "NOT_FOUND");
    assert!(
        missing.message.contains("capturedPromptId"),
        "{}",
        missing.message
    );
    // 缺参数是 INVALID_ARGUMENT（不是 NOT_FOUND）
    assert_eq!(
        call_tool(&state, "comfy_get_run", serde_json::json!({}))
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// `comfy_list_workflows`：与 `GET /api/capture/workflows` **同一个实现**
/// （三份目录来源 + `file:<sha256>` 那把幂等钥匙），只是搬到了工具层。
#[tokio::test]
async fn comfy_list_workflows_reads_the_same_dirs_as_the_endpoint() {
    let state = state();
    let root = state.project_root();
    // 目录来源 ①：配置里生效的输出目录 → 它的父目录就是 ComfyUI 的根
    let workflows = root
        .join("comfyui")
        .join("user")
        .join("default")
        .join("workflows");
    std::fs::create_dir_all(root.join("comfyui").join("output")).unwrap();
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("krea2_demo.json"),
        br#"{"nodes":[],"links":[]}"#,
    )
    .unwrap();
    // 坏 JSON / 非 .json / 空文件：**一份都不许列**（列出来只会让 AI 白跑一趟）
    std::fs::write(workflows.join("broken.json"), b"{ not json").unwrap();
    std::fs::write(workflows.join("empty.json"), b"").unwrap();
    std::fs::write(workflows.join("note.txt"), b"{}").unwrap();
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        output_dir: Some(root.join("comfyui").join("output").display().to_string()),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let out = call_tool(&state, "comfy_list_workflows", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(out.payload["count"], 1, "{}", out.payload);
    assert_eq!(out.payload["files"][0]["name"], "krea2_demo.json");
    assert_eq!(out.payload["files"][0]["format"], "ui");
    assert_eq!(out.payload["files"][0]["inLibrary"], false);
    assert!(
        out.payload["files"][0].get("promptId").is_none(),
        "{}",
        out.payload
    );

    // 按文件名过滤（用户说"krea2 那条"）
    let hit = call_tool(
        &state,
        "comfy_list_workflows",
        serde_json::json!({ "query": "KREA2" }),
    )
    .await
    .unwrap();
    assert_eq!(hit.payload["count"], 1, "大小写不敏感：{}", hit.payload);

    // 入库判定用的是 `file:<sha256>` 这把**与 comfy_load_workflow 相同的**钥匙。
    // 塞一行进去之后 inLibrary 必须翻成 true 并带上 promptId（幂等的前提）。
    let bytes = std::fs::read(workflows.join("krea2_demo.json")).unwrap();
    let sha = comfyhub_server_rs::comfy::comfy_dirs::sha256_hex(&bytes);
    state
        .with(|conn| {
            // `capture_runs.prompt_id` 有外键 → 指向夹具里真实的提示词 10
            conn.execute(
                "INSERT INTO capture_runs (run_key, prompt_id, status, media_count) VALUES (?1, 10, 'success', 0)",
                [format!("file:{sha}")],
            )?;
            Ok(())
        })
        .unwrap();
    let again = call_tool(&state, "comfy_list_workflows", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        again.payload["files"][0]["inLibrary"], true,
        "{}",
        again.payload
    );
    assert_eq!(again.payload["files"][0]["promptId"], 10);

    // 与端点是同一条路：形状逐键相等（各写一份实现就会出现"界面列得出来、AI 说没有"）
    let (status, endpoint) = get_json(router_for(state.clone()), "/api/capture/workflows").await;
    assert_eq!(status, StatusCode::OK);
    let mut via_tool = again.payload.clone();
    let mut via_endpoint = endpoint.clone();
    // 端点默认 limit=50、工具默认 20：这里只有一份，两份都该给同一份
    assert_eq!(
        via_tool["dirs"], via_endpoint["dirs"],
        "目录来源必须完全一致"
    );
    assert_eq!(
        via_tool["files"].as_array().unwrap().len(),
        via_endpoint["files"].as_array().unwrap().len()
    );
    via_tool["count"] = serde_json::json!(0);
    via_endpoint["count"] = serde_json::json!(0);
    assert_eq!(via_tool, via_endpoint, "工具与端点的响应形状必须逐键一致");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn capture_status_and_jobs_ask_the_real_queue() {
    let state = state();
    // ① ComfyUI 不在：如实回 comfyReachable=false，**不是 500**（状态查询失败会让整个页面挂掉）
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();
    let (status_code, down) = get_json(router_for(state.clone()), "/api/capture/status").await;
    assert_eq!(status_code, StatusCode::OK, "{down}");
    assert_eq!(down["comfyReachable"], false);
    assert_eq!(down["queueRunning"], 0);
    assert_eq!(down["queuePending"], 0);
    assert_eq!(down["capturedRuns"], 1, "夹具里有一条 capture_runs");
    assert!(down["recent"].as_array().unwrap().len() <= 20);
    // `comfyUrl` / `enabled` / 统计键都在
    assert_eq!(down["enabled"], true);
    assert_eq!(down["comfyUrl"], "http://127.0.0.1:1");

    // ② ComfyUI 在：队列数与队首标题都要真的读到
    let (url, handle) = fake_comfy_queue(queue_body()).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let (status_code, up) = get_json(router_for(state.clone()), "/api/capture/status").await;
    assert_eq!(status_code, StatusCode::OK, "{up}");
    assert_eq!(up["comfyReachable"], true, "{up}");
    assert_eq!(up["queueRunning"], 1, "{up}");
    assert_eq!(up["queuePending"], 2, "{up}");

    // /jobs 是右侧栏轮询用的那一份：队首标题、submissions（这一场没提交过 → 空数组）。
    // ⚠ 正在跑的那一条会多出第 6 个键 `current`（P4 第 72 步）—— 这里什么都没跑，
    //   所以它**整键不出现**，键集合仍是这 5 个（可空键省略，与 `runningLabel` 同一规则）。
    let (status_code, jobs) = get_json(router_for(state.clone()), "/api/capture/jobs").await;
    assert_eq!(status_code, StatusCode::OK, "{jobs}");
    assert_eq!(
        keys(&jobs),
        vec![
            "comfyReachable",
            "queuePending",
            "queueRunning",
            "runningLabel",
            "submissions"
        ]
    );
    assert_eq!(jobs["queueRunning"], 1);
    assert_eq!(jobs["queuePending"], 2);
    assert_eq!(jobs["runningLabel"], "krea2 出图");
    assert_eq!(
        jobs["submissions"].as_array().unwrap().len(),
        0,
        "这一场没提交过 → 空"
    );

    // ③ 工具层：`comfy_get_status` 按描述真的裁 recent（Kotlin 那边读了但没用），且如实说裁了多少
    let none = call_tool(
        &state,
        "comfy_get_status",
        serde_json::json!({ "recentLimit": 0 }),
    )
    .await
    .unwrap();
    assert!(
        none.payload.get("recent").is_none(),
        "recentLimit=0 → 不带 recent：{:?}",
        none.payload
    );
    assert!(
        none.content.contains("最近捕获记录已省略"),
        "{}",
        none.content
    );

    // ★ `submissions` 必须是**真的**：模型刚 `comfy_submit` 完就来查状态时，看到 `[]` 会以为
    //   自己根本没提交过（Kotlin `comfyStatus` 给的是 `submitter.submissions(5)`）。
    //   这一条就是那条"恒空数组"的防复发线。
    state.submissions.record(serde_json::json!({
        "promptId": "comfy-run-status-1",
        "submittedAt": "2026-09-27T00:00:00Z",
        "status": "running",
    }));
    let small = call_tool(
        &state,
        "comfy_get_status",
        serde_json::json!({ "recentLimit": 5 }),
    )
    .await
    .unwrap();
    assert_eq!(small.payload["comfyReachable"], true);
    assert!(
        small.payload.get("submissions").is_some(),
        "工具带上 submissions（与 Kotlin 同）"
    );
    assert_eq!(
        small.payload["submissions"].as_array().unwrap().len(),
        1,
        "刚提交过就该看得到：{:?}",
        small.payload["submissions"]
    );
    assert_eq!(
        small.payload["submissions"][0]["promptId"],
        "comfy-run-status-1"
    );

    handle.abort();
    std::fs::remove_dir_all(state.project_root()).ok();
}

#[tokio::test]
async fn capture_jobs_and_status_carry_real_progress_and_current_step() {
    let state = state();
    // ⚠ 把 ComfyUI 地址钉死在一个**没人听**的端口上：不钉的话这台机器上真跑着的
    //   8188 会被问一句，队首真有任务时 `runningLabel` 冒出来、键集合就随环境变了
    //   （与上面那条用例同一个理由）。
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();
    // ① 什么都没跑、也没进度：`current` 与 `progress` **整键不出现**
    //    （不做这一步就不知道"省略规则"有没有被写坏成 `null`）
    let (code, jobs) = get_json(router_for(state.clone()), "/api/capture/jobs").await;
    assert_eq!(code, StatusCode::OK);
    assert!(
        jobs.get("current").is_none(),
        "没在跑就不该有 current：{jobs}"
    );
    let (_, status) = get_json(router_for(state.clone()), "/api/capture/status").await;
    assert!(status.get("current").is_none(), "status 同：{status}");

    // ② 提交时登记好"节点 id → 可读名"（`comfy_submit` 手里就有 API 图），并记一条提交
    state.comfy_progress.register_graph(
        PROGRESS_PROMPT_ID,
        &serde_json::json!({
            "3": { "class_type": "KSampler", "_meta": { "title": "采样 KSampler" } },
            "5": { "class_type": "CLIPTextEncode" },
        }),
    );
    state.submissions.record(serde_json::json!({
        "promptId": PROGRESS_PROMPT_ID,
        "submittedBy": "ai",
        "status": "running",
        "submittedAt": "2026-09-27T00:00:00Z",
        "elapsedMs": 1200,
        "mediaIds": [],
    }));

    // ③ ★ 真帧：开始 + 轮到节点 5（**没有** value/max）
    state.comfy_progress.frame(REAL_EXECUTION_START);
    state.comfy_progress.frame(REAL_EXECUTING);
    let (_, jobs) = get_json(router_for(state.clone()), "/api/capture/jobs").await;
    assert_eq!(
        keys(&jobs),
        vec![
            "comfyReachable",
            "current",
            "queuePending",
            "queueRunning",
            "submissions"
        ],
        "正在跑的时候多出 current：{jobs}"
    );
    assert_eq!(jobs["current"]["promptId"], PROGRESS_PROMPT_ID);
    assert_eq!(jobs["current"]["state"], "running");
    assert_eq!(jobs["current"]["nodeId"], "5");
    assert_eq!(
        jobs["current"]["nodeName"], "CLIPTextEncode",
        "可读名来自提交时登记的图"
    );
    assert!(
        jobs["current"].get("percent").is_none(),
        "★ 没有 progress 帧就是**进度未知**，绝不拿'已执行节点数/总节点数'凑一个数：{jobs}"
    );
    assert_eq!(jobs["submissions"][0]["promptId"], PROGRESS_PROMPT_ID);
    assert_eq!(jobs["submissions"][0]["progress"]["nodeId"], "5");
    // `/api/capture/status` 也带上"正在跑的那条"
    let (_, status) = get_json(router_for(state.clone()), "/api/capture/status").await;
    assert_eq!(status["current"]["promptId"], PROGRESS_PROMPT_ID);

    // ④ ✎ 手写帧：经典 `progress`（本机这个版本没走 `hijack_progress` 的 ProgressBar 钩子，
    //    所以没抓到活帧；形状抄自 main.py:432 `{"value","max","prompt_id","node"}`）
    let progress_frame = format!(
        r#"{{"type": "progress", "data": {{"value": 12, "max": 30, "prompt_id": "{PROGRESS_PROMPT_ID}", "node": "3"}}}}"#
    );
    state.comfy_progress.frame(&progress_frame);
    let (_, jobs) = get_json(router_for(state.clone()), "/api/capture/jobs").await;
    assert_eq!(jobs["current"]["percent"], 40, "12/30 = 40%");
    assert_eq!(jobs["current"]["value"], 12);
    assert_eq!(jobs["current"]["max"], 30);
    assert_eq!(jobs["current"]["nodeId"], "3");
    assert_eq!(
        jobs["current"]["nodeName"], "采样 KSampler",
        "`_meta.title` 优先于 class_type"
    );
    assert_eq!(
        jobs["submissions"][0]["progress"]["percent"], 40,
        "提交条目上也要有同一份"
    );

    // ⑤ 模型那条路（`comfy_get_status`）：要说得出"跑到哪一步了"
    let tool = call_tool(
        &state,
        "comfy_get_status",
        serde_json::json!({ "recentLimit": 0 }),
    )
    .await
    .unwrap();
    assert_eq!(tool.payload["current"]["percent"], 40, "{:?}", tool.payload);
    let step = tool.payload["currentStep"]
        .as_str()
        .expect("要有一句人话的当前步骤");
    assert!(step.contains("40%"), "{step}");
    assert!(step.contains("采样 KSampler"), "{step}");
    assert_eq!(
        tool.payload["progressConnected"], false,
        "WS 没连上就如实说没连上（这里没起它）"
    );

    // ⑥ ★ 真帧：`execution_success` → current 清掉、进度留痕但**不许抬到 100**
    state.comfy_progress.frame(REAL_EXECUTION_SUCCESS);
    let (_, jobs) = get_json(router_for(state.clone()), "/api/capture/jobs").await;
    assert!(
        jobs.get("current").is_none(),
        "跑完之后就没有'正在跑的'了：{jobs}"
    );
    assert_eq!(jobs["submissions"][0]["progress"]["state"], "success");
    assert_eq!(
        jobs["submissions"][0]["progress"]["percent"], 40,
        "★ 终态不把 percent 编成 100 —— 界面按 state 显示'已完成'"
    );
    assert!(
        jobs["submissions"][0]["progress"].get("nodeId").is_none(),
        "跑完了就不该还挂着某个节点"
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

#[tokio::test]
async fn poll_captures_a_run_into_prompts_and_media() {
    use comfyhub_server_rs::comfy::capture_ingest;
    let state = state();
    let root = state.project_root();
    // ① 本地 output 目录里**真的有**那份产物（这条路不下载，直接收）
    let output_dir = root.join("comfyui").join("output");
    std::fs::create_dir_all(&output_dir).unwrap();
    let media_bytes = tiny_png(8, 6);
    std::fs::write(output_dir.join("krea2_00001_.png"), &media_bytes).unwrap();

    let history: &'static str =
        Box::leak(history_with_image("krea2_00001_.png", "uuid-run-1").into_boxed_str());
    let view: &'static [u8] = Box::leak(media_bytes.clone().into_boxed_slice());
    let (url, handle) = fake_comfy(history, view).await;

    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        output_dir: Some(output_dir.display().to_string()),
        auto_tag: "ComfyUI".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    // ② 轮询一次：应当收下 1 次运行、1 个产物
    let config = comfyhub_server_rs::comfy::capture::capture_config(&state).unwrap();
    let result = capture_ingest::poll_once(&state, &config).await;
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["checked"], 1, "{result}");
    assert_eq!(result["newRuns"], 1, "{result}");
    assert_eq!(result["newMedia"], 1, "{result}");

    // ③ 运行记录：success + prompt_id + media_count + 标题（seed 进标题）
    let (status, prompt_id, media_count, title): (String, i64, i64, String) = state
        .with(|conn| {
            conn.query_row(
                "SELECT status, prompt_id, media_count, title FROM capture_runs WHERE run_key = 'uuid-run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .unwrap();
    assert_eq!(status, "success");
    assert_eq!(media_count, 1);
    assert!(title.contains("seed 4242"), "标题要带 seed：{title}");

    // ④ 提示词：字段真的解析进去了（这条链就是第 43 步第一半的产物）
    type PromptFields = (
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let (positive, negative, checkpoint, seed, steps, width): PromptFields = state
        .with(|conn| {
            conn.query_row(
                "SELECT positive_prompt, negative_prompt, checkpoint, seed, steps, width FROM prompts WHERE id = ?1",
                [prompt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
        })
        .unwrap();
    assert_eq!(positive, "1girl, ink painting");
    assert_eq!(negative.as_deref(), Some("lowres"));
    assert_eq!(checkpoint, None, "这张图里没有 checkpoint 节点");
    assert_eq!(seed, Some(4242));
    assert_eq!(steps, Some(12));
    assert_eq!(width, Some(768));
    // source / source_ref 是"同一次生成只建一条提示词"的钥匙
    let (source, source_ref, loras, extra): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = state
        .with(|conn| {
            conn.query_row(
                "SELECT source, source_ref, loras, extra_params FROM prompts WHERE id = ?1",
                [prompt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .unwrap();
    assert_eq!(source.as_deref(), Some("ComfyUI"));
    assert_eq!(source_ref.as_deref(), Some("uuid-run-1"));
    // ⚠ 空集合的写法必须与 Kotlin 一致：loras = `[]`、extra = `{}`（不是 NULL）
    assert_eq!(
        loras.as_deref(),
        Some("[]"),
        "空 LoRA 列表要写 []（真库实测如此）"
    );
    assert!(extra.as_deref().unwrap_or("").starts_with('{'), "{extra:?}");

    // ⑤ 产物：真的落进 storage/media、挂在提示词上、带了标签与 sha256
    let asset: (i64, String, i64, Option<i64>, String) = state
        .with(|conn| {
            conn.query_row(
                "SELECT id, kind, size_bytes, prompt_id, stored_name FROM media_assets ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
        })
        .unwrap();
    assert_eq!(asset.1, "IMAGE");
    assert_eq!(asset.2, media_bytes.len() as i64);
    assert_eq!(asset.3, Some(prompt_id), "产物必须挂在这次运行的提示词上");
    assert!(
        state.media_dir().join(&asset.4).is_file(),
        "产物要真的落进 storage/media"
    );
    // 图片顺手写缩略图（画廊列表用它）
    assert!(comfyhub_server_rs::media::thumbs::thumb_path(state.media_dir(), asset.0).is_file());
    let tag_count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM media_tags WHERE media_id = ?1",
                [asset.0],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(tag_count, 1, "autoTag（ComfyUI）要挂上");

    // ⑥ 幂等：再轮询一次**什么都不该变**（success 是终态）
    let prompts_before: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(prompts_before, 3, "夹具 2 条 + 这次捕获 1 条");
    let again = capture_ingest::poll_once(&state, &config).await;
    assert_eq!(again["checked"], 0, "{again}");
    assert_eq!(again["newRuns"], 0, "{again}");
    let prompts_after: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0)))
        .unwrap();
    let media_after: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM media_assets", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(prompts_after, prompts_before, "不该重复建提示词");
    // 夹具里本来就有 1 条产物（`a.png`）+ 这次捕获的 1 条
    assert_eq!(media_after, 2, "不该重复入库产物");

    handle.abort();
    std::fs::remove_dir_all(&root).ok();
}

/// 产物**本地没有**时走 HTTP 下载兜底（`GET /view`），而且 `empty` 能重试。
#[tokio::test]
async fn poll_downloads_missing_outputs_and_retries_empty_runs() {
    use comfyhub_server_rs::comfy::capture_ingest;
    let state = state();
    let root = state.project_root();
    let output_dir = root.join("comfyui").join("output");
    std::fs::create_dir_all(&output_dir).unwrap();
    let media_bytes = tiny_png(4, 4);
    let view: &'static [u8] = Box::leak(media_bytes.clone().into_boxed_slice());

    // 第一轮：/history 有记录但没有 outputs → 收成 `empty`（**可重试**，不是终态）
    let empty_history: &'static str = Box::leak(
        serde_json::json!({
            "uuid-run-2": {
                "prompt": {"1":{"class_type":"KSampler","inputs":{"seed":7,"steps":4,"cfg":1}}},
                "outputs": {},
                "status": {"completed": true, "status_str": "success"}
            }
        })
        .to_string()
        .into_boxed_str(),
    );
    let (url, handle) = fake_comfy(empty_history, view).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        // ⚠ 故意把 outputDir 指向一个**没有那份文件**的目录 → 逼出 HTTP 下载兜底
        output_dir: Some(output_dir.display().to_string()),
        auto_tag: "ComfyUI".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let config = comfyhub_server_rs::comfy::capture::capture_config(&state).unwrap();
    let first = capture_ingest::poll_once(&state, &config).await;
    assert_eq!(first["checked"], 1, "{first}");
    assert_eq!(first["newRuns"], 0, "没有产物 → 不算新运行");
    let status: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT status FROM capture_runs WHERE run_key='uuid-run-2'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(status, "empty");
    // 产物晚到之后**必须能重收**（当终态就会出现"AI 说生成了、画廊里却没有"）
    let claim = capture_ingest::can_reclaim("empty", 1, false);
    assert!(claim, "empty 必须可重试");

    // 第二轮：同一个 run 现在有产物了（且本地没有 → 走 /view 下载）
    handle.abort();
    let late_history: &'static str = Box::leak(
        serde_json::json!({
            "uuid-run-2": {
                "prompt": {
                    "3": {"class_type":"KSampler","inputs":{"seed":99,"steps":8,"cfg":3,
                          "positive":["6",0],"latent_image":["5",0]}},
                    "5": {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}},
                    "6": {"class_type":"CLIPTextEncode","inputs":{"text":"late arrival"}},
                    "9": {"class_type":"SaveImage","inputs":{"filename_prefix":"x"}}
                },
                "outputs": {"9": {"images":[{"filename":"late.png","type":"output"}]}},
                "status": {"completed": true, "status_str": "success"}
            }
        })
        .to_string()
        .into_boxed_str(),
    );
    let (url2, handle2) = fake_comfy(late_history, view).await;
    let mut config2 = comfyhub_server_rs::comfy::capture::capture_config(&state).unwrap();
    config2.comfy_url = url2.clone();
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config2).unwrap();

    let config2 = comfyhub_server_rs::comfy::capture::capture_config(&state).unwrap();
    let second = capture_ingest::poll_once(&state, &config2).await;
    assert_eq!(
        second["checked"], 1,
        "empty 的运行必须能被重新抢占：{second}"
    );
    assert_eq!(
        second["newMedia"], 1,
        "下载兜底要真的把产物收进来：{second}"
    );
    let (status, media_count): (String, i64) = state
        .with(|conn| {
            conn.query_row(
                "SELECT status, media_count FROM capture_runs WHERE run_key='uuid-run-2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(status, "success");
    assert_eq!(media_count, 1);
    // 下载来的那份是临时文件，收完必须清掉（别在 storage/tmp 里攒垃圾）
    let tmp_left = std::fs::read_dir(root.join("tmp"))
        .map(|iter| iter.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    assert_eq!(tmp_left, 0, "下载的临时文件收完要删掉");

    handle2.abort();
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn load_api_workflow_file_is_idempotent_and_feeds_api_graph() {
    let state = state();
    let root = state.project_root();
    std::fs::create_dir_all(root.join("comfyui")).unwrap();
    let file = root.join("comfyui").join("api_flow.json");
    std::fs::write(&file, api_workflow_file()).unwrap();

    let out = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string() }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["format"], "api");
    assert_eq!(out.payload["runnable"], true, "{:?}", out.payload);
    assert_eq!(out.payload["nodeCount"], 4);
    assert!(
        out.payload["runKey"].as_str().unwrap().starts_with("file:"),
        "{:?}",
        out.payload
    );
    let prompt_id = out.payload["promptId"].as_i64().unwrap();
    // 解析出来的字段真的进了库（第一半那份 GraphParse）
    let (source, source_ref, seed): (Option<String>, Option<String>, Option<i64>) = state
        .with(|conn| {
            conn.query_row(
                "SELECT source, source_ref, seed FROM prompts WHERE id = ?1",
                [prompt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .unwrap();
    assert_eq!(source.as_deref(), Some("ComfyUI-File"));
    assert!(source_ref.as_deref().unwrap().starts_with("file:"));
    assert_eq!(seed, Some(11));
    // `nodes` 摘要把"能覆盖的参数"与"连线"分开（键顺序 = 文件里的文档顺序）
    assert_eq!(out.payload["nodes"][0]["classType"], "KSampler");
    let latent = out.payload["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["classType"] == "EmptyLatentImage")
        .unwrap();
    assert_eq!(latent["widgets"], serde_json::json!(["width", "height"]));
    assert_eq!(latent["linked"], serde_json::json!([]));
    // **同一个路径再读一次：幂等**（不重复建提示词、promptId 不变）
    let again = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string() }),
    )
    .await
    .unwrap();
    assert_eq!(
        again.payload["promptId"], prompt_id,
        "同一份文件必须复用同一条提示词"
    );
    let count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM prompts WHERE source='ComfyUI-File'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1);

    // 库里那条提示词的 **API 图**能取回来（comfy_submit 只认它，R10 的前提）
    let (status, graph) = get_json(
        router_for(state.clone()),
        &format!("/api/prompts/{prompt_id}/api-graph"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(graph["3"]["class_type"], "KSampler");

    // 用户把这条提示词删掉之后**必须还能重新导入**（`prompt_id` 被置空那条路）
    state
        .with(|conn| {
            conn.execute("DELETE FROM prompts WHERE id = ?1", [prompt_id])?;
            Ok(())
        })
        .unwrap();
    let reloaded = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string() }),
    )
    .await
    .unwrap();
    assert_ne!(
        reloaded.payload["promptId"], prompt_id,
        "删掉之后要重新建一条（不是永远说「正在加载」）"
    );
    assert_eq!(reloaded.payload["runnable"], true);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn load_ui_workflow_uses_object_info_and_reports_gaps() {
    let state = state();
    let root = state.project_root();
    let output = root.join("comfyui");
    std::fs::create_dir_all(&output).unwrap();
    let file = output.join("ui_flow.json");
    std::fs::write(&file, ui_workflow_file()).unwrap();

    let history: &'static str = Box::leak(r#"{}"#.to_string().into_boxed_str());
    let view: &'static [u8] = Box::leak(Vec::new().into_boxed_slice());
    let info: &'static str = Box::leak(object_info_json().into_boxed_str());
    let (url, handle) = fake_comfy_full(history, view, info).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    // ① 默认档：撞上不认识的前端节点 → **如实拒绝**（UNSUPPORTED_NODES），不许猜着提交
    let refused = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string() }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "UNSUPPORTED_NODES", "{}", refused.message);
    assert!(
        refused.message.contains("Weird Frontend Thing"),
        "{}",
        refused.message
    );

    // ② tolerateUnsupported=true：摘掉它、把缺口清单给出来
    let out = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string(), "tolerateUnsupported": true, "includeGraph": true }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["format"], "ui");
    assert_eq!(
        out.payload["nodeCount"], 3,
        "前端节点被摘掉：{:?}",
        out.payload
    );
    assert_eq!(out.payload["runnable"], false, "有缺口就不算能跑");
    assert!(
        out.payload["unsupportedNodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("Weird")),
        "{:?}",
        out.payload
    );
    // ⚠ **第 68 步之前**这里还有一句"本后端**还没实现**「模拟用户行为」"的补白。
    //   现在那个能力真的做了（`workflow_frontend.rs` = Kotlin `FrontendNodes.kt` 的对照），
    //   所以两件事都要钉住：
    //   ① 那句"还没做"的补白**不许再出现**（工具描述里写的能力已经兑现）；
    //   ② 显示节点要**真的被丢掉**，而且如实列在 `frontendEmulated` 里（用户看得到才敢信）。
    let warnings: Vec<String> = out.payload["warnings"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !warnings.iter().any(|w| w.contains("还没实现")),
        "「模拟用户行为」已经实现，不该再补那句白：{warnings:?}"
    );
    assert!(
        out.payload["frontendEmulated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v
                .as_str()
                .unwrap()
                .contains("5 PreviewImage（显示节点，已忽略）")),
        "显示节点被丢掉要如实列出来：{:?}",
        out.payload["frontendEmulated"]
    );
    assert!(
        out.payload["frontendHint"]
            .as_str()
            .unwrap_or_default()
            .contains("emulateFrontend=false"),
        "改了前端的东西就要告诉模型怎么关掉：{:?}",
        out.payload["frontendHint"]
    );
    // 关掉这个开关就**一个字都不改**：显示节点回到 unsupportedNodes 里，清单也空着
    let raw = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string(), "tolerateUnsupported": true,
                            "emulateFrontend": false }),
    )
    .await
    .unwrap();
    assert!(
        raw.payload["unsupportedNodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("PreviewImage")),
        "关着就不许动它：{:?}",
        raw.payload["unsupportedNodes"]
    );
    assert!(
        raw.payload["frontendEmulated"].is_null(),
        "{:?}",
        raw.payload
    );
    // 转换后的图里控件是按声明顺序贴回去的
    let graph = &out.payload["apiGraph"];
    assert_eq!(graph["3"]["class_type"], "KSampler");
    assert_eq!(graph["3"]["inputs"]["seed"], 777);
    assert_eq!(
        graph["3"]["inputs"]["positive"],
        serde_json::json!(["1", 0])
    );
    assert_eq!(graph["2"]["inputs"]["width"], 512);
    // 缺口清单：正面提示词那一路接上了，但 `negative` / `model` 还空着
    assert!(
        out.payload["openInputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("3.negative")),
        "{:?}",
        out.payload["openInputs"]
    );
    let prompt_id = out.payload["promptId"].as_i64().unwrap();
    let (status, api_graph) = get_json(
        router_for(state.clone()),
        &format!("/api/prompts/{prompt_id}/api-graph"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{api_graph}");
    assert_eq!(
        api_graph["3"]["inputs"]["steps"], 6,
        "转化后的图要真的存进 capture_runs.raw"
    );

    handle.abort();
    std::fs::remove_dir_all(&root).ok();
}

/// ## 模拟用户行为 ##：走**工具这一整条路**（不只是转换函数），钉住结果里那几个字段名。
///
/// 为什么单独一条：`frontendBroadcast` / `frontendInjected` 这些键是**给模型看的契约**
/// （Kotlin `AiWorkflowSearch` 里那几个 `put`）—— 单元测试只验 `Converted` 的字段，
/// 验不到"落到工具结果里叫什么名字"。
#[tokio::test]
async fn load_ui_workflow_emulates_frontend_and_injects_a_save_node() {
    let state = state();
    let root = state.project_root();
    let output = root.join("comfyui");
    std::fs::create_dir_all(&output).unwrap();
    let file = output.join("ui_broadcast.json");
    std::fs::write(&file, ui_workflow_broadcast_file()).unwrap();

    let history: &'static str = Box::leak(r#"{}"#.to_string().into_boxed_str());
    let view: &'static [u8] = Box::leak(Vec::new().into_boxed_slice());
    let info: &'static str = Box::leak(object_info_for_broadcast().into_boxed_str());
    let (url, handle) = fake_comfy_full(history, view, info).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let out = call_tool(
        &state,
        "comfy_load_workflow",
        serde_json::json!({ "path": file.display().to_string(), "includeGraph": true }),
    )
    .await
    .unwrap();

    // 广播 + 补 SaveImage 之后，这张图**真的能跑**了（默认档不再拒）
    assert_eq!(out.payload["runnable"], true, "{:?}", out.payload);
    assert!(
        out.payload["unsupportedNodes"].is_null(),
        "{:?}",
        out.payload["unsupportedNodes"]
    );
    assert_eq!(
        out.payload["nodeCount"], 6,
        "5 个真节点 + 补出来的 SaveImage：{:?}",
        out.payload
    );

    let broadcast: Vec<&str> = out.payload["frontendBroadcast"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        broadcast.len(),
        2,
        "两条空的 CONDITIONING 都要被填上：{broadcast:?}"
    );
    assert!(
        broadcast
            .iter()
            .any(|line| line.starts_with("3.positive ← 节点 14")),
        "{broadcast:?}"
    );
    assert!(
        broadcast
            .iter()
            .any(|line| line.starts_with("3.negative ← 节点 14")),
        "{broadcast:?}"
    );
    assert!(
        out.payload["frontendEmulated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("广播 2 处")),
        "{:?}",
        out.payload["frontendEmulated"]
    );

    // 补出来的输出节点接在**真正产出 IMAGE 的那个节点**上（`VAEDecode` 是 5 号）
    let injected: Vec<&str> = out.payload["frontendInjected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert!(injected[0].contains("原本没有输出节点"), "{injected:?}");
    let graph = &out.payload["apiGraph"];
    assert_eq!(graph["6"]["class_type"], "SaveImage");
    assert_eq!(graph["6"]["inputs"]["images"], serde_json::json!(["5", 0]));

    // 灌完之后 openInputs 要重算：那两条线已经接好了，不该再让模型去补
    let open_inputs = out.payload["openInputs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !open_inputs
            .iter()
            .any(|v| v.as_str().unwrap().contains("3.positive")),
        "{:?}",
        out.payload["openInputs"]
    );

    handle.abort();
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn comfy_check_inputs_reports_gaps_and_reads_the_attachment_flag() {
    let state = state();
    let mut session = comfyhub_server_rs::tools::tools::ToolSession::default();

    // ① 没带图：LoadImage 那条必须报出来，并且要挡住"带着缺口硬提交"这条路
    let out = call_tool_with_session(
        &state,
        "comfy_check_inputs",
        serde_json::json!({ "graph": graph_with_image_gap() }),
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(out.payload["missingCount"], 1, "{}", out.payload);
    assert_eq!(out.payload["needsExternalAsset"], true, "{}", out.payload);
    assert_eq!(out.payload["nodeCount"], 4, "SaveImage 的 images 不是缺口");
    assert_eq!(out.payload["missing"][0]["ref"], "89.image");
    assert_eq!(out.payload["missing"][0]["kind"], "image");
    // `currentValue` 是**原值的 JSON 文本**（Kotlin `value.toString()`），所以带引号
    assert_eq!(
        out.payload["missing"][0]["currentValue"],
        "\"4789521234.jpg\""
    );
    assert!(out.content.contains("等着外部输入"), "{}", out.content);
    assert!(out.content.contains("不要硬提交"), "{}", out.content);
    // 没有 `title` 时**省掉键**（Kotlin 手拼 JSON 的取舍），不是写 null
    assert!(
        out.payload["missing"][0].get("title").is_none(),
        "{}",
        out.payload
    );

    // ② 本次带了图：同一个位置改列 `suppliedByAttachment`（**不要再问用户要图**）
    let mut with_image = comfyhub_server_rs::tools::tools::ToolSession::default();
    with_image.has_image_attachment = true;
    let out = call_tool_with_session(
        &state,
        "comfy_check_inputs",
        serde_json::json!({ "graph": graph_with_image_gap() }),
        &mut with_image,
    )
    .await
    .unwrap();
    assert_eq!(out.payload["missingCount"], 0, "{}", out.payload);
    assert_eq!(out.payload["needsExternalAsset"], false);
    assert_eq!(
        out.payload["suppliedByAttachment"][0],
        "89.image（本次附件可以投放）"
    );
    assert!(out.content.contains("不必问用户"), "{}", out.content);

    // ③ 字符串形态的图也认（模型从别处粘贴时经常带上引号）
    let out = call_tool_with_session(
        &state,
        "comfy_check_inputs",
        serde_json::json!({ "graph": graph_with_image_gap().to_string() }),
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(out.payload["missingCount"], 1);

    // ④ 界面格式（nodes/links）**不是** API 图：如实拒绝，并指路 comfy_load_workflow
    let refused = call_tool_with_session(
        &state,
        "comfy_check_inputs",
        serde_json::json!({ "graph": {"nodes": [], "links": []} }),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "INVALID_ARGUMENT");
    assert!(
        refused.message.contains("comfy_load_workflow"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("不是一份 API 格式节点图"),
        "{}",
        refused.message
    );
    // 连 graph 都没给也是同一档（不是 500，也不是"什么都没缺"）
    let missing = call_tool_with_session(
        &state,
        "comfy_check_inputs",
        serde_json::json!({}),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "INVALID_ARGUMENT");

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// `comfy_fit_workflow`：图里点名的模型 ↔ **实例枚举**出来的清单，配上就是配上
/// （`/object_info` 那一条枚举是"它能加载什么"的唯一权威）。
#[tokio::test]
async fn comfy_fit_workflow_pairs_graph_models_with_the_instance_inventory() {
    let state = state();
    let history: &'static str = Box::leak(r#"{}"#.to_string().into_boxed_str());
    let view: &'static [u8] = Box::leak(Vec::new().into_boxed_slice());
    let info: &'static str = Box::leak(FIT_OBJECT_INFO.to_string().into_boxed_str());
    let (url, handle) = fake_comfy_full(history, view, info).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let graph = serde_json::json!({
        "1": {"class_type": "CheckpointLoaderSimple",
              "inputs": {"ckpt_name": "krea2_turbo_int8.safetensors"},
              "_meta": {"title": "底模"}},
        "3": {"class_type": "KSampler", "inputs": {"steps": 20}},
        "9": {"class_type": "SomeMissingNode", "inputs": {}}
    });

    let out = call_tool(
        &state,
        "comfy_fit_workflow",
        serde_json::json!({ "graph": graph.clone() }),
    )
    .await
    .unwrap();
    let payload = &out.payload;
    assert_eq!(payload["inventorySource"], "object_info", "{payload}");
    assert_eq!(payload["required"], 1);
    assert_eq!(payload["exact"], 1);
    assert_eq!(payload["missing"], 0);
    assert_eq!(payload["onDiskOnly"], 0);
    let item = &payload["models"][0];
    assert_eq!(item["overrideKey"], "1.ckpt_name");
    assert_eq!(item["wanted"], "krea2_turbo_int8.safetensors");
    assert_eq!(item["folder"], "checkpoints");
    assert_eq!(item["title"], "底模");
    assert_eq!(item["status"], "exact");
    assert_eq!(item["safeToAutoApply"], true);
    // 候选必须带上"实例能不能加载"这一位（`/object_info` 来的当然能）
    assert_eq!(
        item["candidates"][0]["name"],
        "krea2_turbo_int8.safetensors"
    );
    assert_eq!(item["candidates"][0]["visibleToInstance"], true);
    // 同名 → 没有任何键要改（`suggestedOverrides` 是**省掉键**，不是空对象）
    assert!(payload.get("suggestedOverrides").is_none(), "{payload}");
    // 缺节点与缺模型**分开报**（缺的是自定义节点，不是模型）
    assert_eq!(payload["missingNodes"][0], "SomeMissingNode", "{payload}");
    assert!(out.content.contains("SomeMissingNode"), "{}", out.content);
    assert!(out.content.contains("节点类"), "{}", out.content);
    assert!(out.content.contains("不用改任何键"), "{}", out.content);
    assert!(out.content.contains("只报不改"), "{}", out.content);

    // 界面格式同样要被挡回来（这条与 `check_inputs` 是同一条判据）
    let refused = call_tool(
        &state,
        "comfy_fit_workflow",
        serde_json::json!({ "graph": {"nodes": []} }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "INVALID_ARGUMENT");
    assert!(
        refused.message.contains("不是一份 API 格式节点图"),
        "{}",
        refused.message
    );

    handle.abort();
    std::fs::remove_dir_all(state.project_root()).ok();
}

// ===========================================================================
//  审核 + 摘拼两件（P4 第 63 步）：comfy_audit_workflow / comfy_prune_workflow
//
//  这一对工具是用户 2026-09-22「让 AI 具有审核本地工作流的能力」的落点。
//  端到端要盯的是两件**只能在这一层验**的事：
//   ① `comfy_audit_workflow` 的"必跑不了"真的能进提交闸门（`blockingRefusal`），
//      而参数 / 显存 / 提示词风格**只提醒不拦**；
//   ② `comfy_prune_workflow` 拿不到 `/object_info` 时**只摘不拼**（乱接一根线的代价
//      是 ComfyUI 回一句看不懂的报错，用户根本查不出来）。
// ===========================================================================

/// `comfy_audit_workflow`：七项检查只报事实；`/object_info` 拿不到时**不判**本机缺节点。
#[tokio::test]
async fn comfy_audit_workflow_reports_facts_and_gates_only_blocking_items() {
    let state = state();
    let history: &'static str = Box::leak(r#"{}"#.to_string().into_boxed_str());
    let view: &'static [u8] = Box::leak(Vec::new().into_boxed_slice());
    let info: &'static str = Box::leak(FIT_OBJECT_INFO.to_string().into_boxed_str());
    let (url, handle) = fake_comfy_full(history, view, info).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    // 一份"处处有问题"的图：前端节点 + 版本不兼容 + 本机没有的类 + 固定 seed/前缀 +
    // 参数偏离官方档位（每一个都对应 `WorkflowAuditTest` 里那条现场）。
    let graph = serde_json::json!({
        "1": {"class_type": "CheckpointLoaderSimple",
              "inputs": {"ckpt_name": "krea2_turbo_int8.safetensors"}},
        "5": {"class_type": "KSampler",
              "inputs": {"steps": 30, "cfg": 4.0, "sampler_name": "ddpm", "seed": 7}},
        "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "same"}},
        "10": {"class_type": "Label (rgthree)", "inputs": {}},
        "94": {"class_type": "WidgetToString", "inputs": {}},
        "9": {"class_type": "SomeMissingNode", "inputs": {}}
    });
    let out = call_tool(
        &state,
        "comfy_audit_workflow",
        serde_json::json!({ "graph": graph.clone() }),
    )
    .await
    .unwrap();
    let payload = &out.payload;
    assert_eq!(payload["nodeCount"], 6, "{payload}");
    assert_eq!(
        payload["block"], 3,
        "前端节点 / 版本不兼容 / 本机没有的类：{payload}"
    );
    assert_eq!(payload["warn"], 2, "固定 seed+前缀 / 参数档位：{payload}");
    assert_eq!(payload["info"], 0);
    let checks: Vec<&str> = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert_eq!(
        checks,
        vec![
            "frontendNodes",
            "unknownNodes",
            "nodeVersions",
            "outputName",
            "params"
        ],
        "七项检查的**报出顺序**也要与 Kotlin 一致：{payload}"
    );
    // 正文：结论先行 + 修法跟着走（模型照它决定下一步）
    assert!(
        out.content.contains("审核结论（6 个节点）"),
        "{}",
        out.content
    );
    assert!(out.content.contains("**跑不起来 3 处**"), "{}", out.content);
    assert!(
        out.content.contains("🔴 [frontendNodes]"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("comfy_prune_workflow"),
        "要指路「摘节点」：{}",
        out.content
    );
    assert!(out.content.contains("只报不改"), "{}", out.content);
    // "必跑不了"那三条要能被提交闸门直接用（同一条判据的纯函数半边在
    // `workflow_audit::Report::blocking_refusal` 上有单测）
    let frontend = &payload["findings"][0];
    assert_eq!(frontend["nodes"][0], "10");
    assert!(
        frontend["detail"].as_str().unwrap().contains("0.01"),
        "{frontend}"
    );

    // 每个模型的检查都"拿不准就不说"：换掉 /object_info（ComfyUI 不在）→ **不判**本机缺节点
    let down = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, down).unwrap();
    let offline = call_tool(
        &state,
        "comfy_audit_workflow",
        serde_json::json!({ "graph": graph.clone() }),
    )
    .await
    .unwrap();
    let checks: Vec<&str> = offline.payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert!(
        !checks.contains(&"unknownNodes"),
        "拿不到 /object_info 时不许说「本机没有这个节点类」：{checks:?}"
    );
    assert!(
        checks.contains(&"frontendNodes"),
        "纯前端节点是**图自己**的问题，照样要报：{checks:?}"
    );

    // 界面格式（nodes/links）不是 API 图：如实拒绝，并指路 comfy_load_workflow
    let refused = call_tool(
        &state,
        "comfy_audit_workflow",
        serde_json::json!({ "graph": {"nodes": []} }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "INVALID_ARGUMENT");
    assert!(
        refused.message.contains("comfy_load_workflow"),
        "{}",
        refused.message
    );
    // 连 graph 都没给也是同一档
    let missing = call_tool(&state, "comfy_audit_workflow", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "INVALID_ARGUMENT");

    handle.abort();
    std::fs::remove_dir_all(state.project_root()).ok();
}

/// `comfy_prune_workflow` 的三条路：真摘（带审计清单）/ `analyze=true`（一个字都不改）/
/// 什么都不要求（`INVALID_ARGUMENT` + 指路 analyze）。
#[tokio::test]
async fn comfy_prune_workflow_prunes_analyzes_and_refuses_empty_requests() {
    let state = state();

    // 一份"文生图 + 参考图分支"（Kotlin `WorkflowPruneTest.withReferenceBranch` 的搬运）
    let with_reference = serde_json::json!({
        "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
        "4": {"class_type":"LoraLoader","inputs":{"model":["1",0],"clip":["1",1],"lora_name":"x.safetensors"}},
        "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["4",1]}},
        "7": {"class_type":"CLIPTextEncode","inputs":{"text":"bad","clip":["4",1]}},
        "89": {"class_type":"LoadImage","inputs":{"image":"478952.jpg","upload":"image"}},
        "92": {"class_type":"ControlNetApplyAdvanced","inputs":{"positive":["6",0],"negative":["7",0],"control_net":["91",0],"image":["89",0],"strength":0.8}},
        "91": {"class_type":"ControlNetLoader","inputs":{"control_net_name":"canny.safetensors"}},
        "3": {"class_type":"KSampler","inputs":{"model":["4",0],"positive":["92",0],"negative":["92",1],"latent_image":["5",0],"seed":1,"steps":20,"cfg":7.0}},
        "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024,"batch_size":1}},
        "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
        "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}}
    });

    // ① 真摘：返回改好的**内联图** + 审计清单；`runnable` 如实
    let out = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": with_reference, "removeFrom": ["89.image"] }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["ok"], true, "{:?}", out.payload);
    assert_eq!(out.payload["runnable"], true, "{:?}", out.payload);
    assert!(
        !out.payload["removed"].as_array().unwrap().is_empty(),
        "摘了什么要如实列出来"
    );
    let graph_out = &out.payload["graph"];
    assert!(graph_out.get("89").is_none(), "点名要摘的 LoadImage 没了");
    assert!(graph_out.get("10").is_some(), "输出节点必须还在");
    // 92 还在（它还有消费者）→ 采样器的线不动；这是"只摘那根线"的那条判据
    assert_eq!(graph_out["3"]["inputs"]["positive"][0], "92");
    assert_eq!(
        graph_out["92"]["inputs"].get("image"),
        None,
        "被摘的那根线要真的没了"
    );
    assert!(
        out.payload["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("92.image")),
        "哪根线断了要说清楚：{:?}",
        out.payload["warnings"]
    );
    assert!(
        out.content.contains("内联"),
        "要如实说清 graph 是内联图：{}",
        out.content
    );

    // ② `analyze=true`：**一个字都不改**（没有 graph 键），并且要求先问用户
    let dead = serde_json::json!({
        "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
        "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["1",1]}},
        "3": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["6",0],"latent_image":["5",0],"seed":1}},
        "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024}},
        "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
        "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}},
        "70": {"class_type":"UpscaleModelLoader","inputs":{"model_name":"4x.pth"}},
        "71": {"class_type":"ImageUpscaleWithModel","inputs":{"upscale_model":["70",0],"image":["9",0]}},
        "80": {"class_type":"FaceSwap","inputs":{"image":["71",0],"face":["81",0]}},
        "81": {"class_type":"LoadImage","inputs":{"image":"face.png"}},
        "99": {"class_type":"Note","inputs":{"text":"还没接完的备注"}}
    });
    let out = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": dead.clone(), "analyze": true }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["ok"], true);
    assert!(
        out.payload.get("graph").is_none(),
        "analyze 绝不能带回改过的图：{}",
        out.payload
    );
    assert_eq!(out.payload["totalUnused"], 4, "{}", out.payload);
    assert_eq!(out.payload["branches"][0]["removeFrom"], "80");
    assert_eq!(out.payload["isolated"][0]["id"], "99");
    let hint = out.payload["hint"].as_str().unwrap();
    assert!(hint.contains("先问用户"), "必须要求先确认：{hint}");
    assert!(hint.contains("还没接完"), "孤立节点不要删：{hint}");
    // 正文就是同一份 JSON（模型读正文、界面读 meta）
    assert_eq!(out.content, out.payload.to_string());

    // ③ 什么都不要求、也没开 analyze → `INVALID_ARGUMENT`，并且指路 analyze
    let refused = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": with_reference }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "INVALID_ARGUMENT");
    assert!(
        refused.message.contains("analyze"),
        "要告诉模型下一步怎么走：{}",
        refused.message
    );

    // ④ 界面格式同样要被挡回来
    let refused = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": {"nodes": []} }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "INVALID_ARGUMENT");
    assert!(
        refused.message.contains("不是一份 API 格式节点图"),
        "{}",
        refused.message
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// `comfy_prune_workflow` 的拼装：**拿不到 `/object_info` 就只摘不拼**（如实报原因），
/// 拿得到就插在那根线上（`SageAttention` 插在 `3.model`，原来的上游接回它自己）。
#[tokio::test]
async fn comfy_prune_workflow_only_prunes_when_the_instance_is_unreachable() {
    let state = state();
    let graph = serde_json::json!({
        "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
        "4": {"class_type":"LoraLoader","inputs":{"model":["1",0],"clip":["1",1],"lora_name":"x.safetensors"}},
        "3": {"class_type":"KSampler","inputs":{"model":["4",0],"steps":20}},
        "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0]}},
        "10": {"class_type":"SaveImage","inputs":{"images":["9",0]}}
    });
    let inject = serde_json::json!([{"at":"3.model","classType":"SageAttention"}]);

    // ① ComfyUI 不在 → `ok=false` + `COMFY_UNREACHABLE`，但**摘除部分照样给出来**
    let down = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, down).unwrap();
    let out = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": graph.clone(), "removeFrom": ["1"], "injections": inject }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["ok"], false, "{:?}", out.payload);
    assert_eq!(out.payload["reason"], "COMFY_UNREACHABLE");
    assert!(
        out.payload.get("detachedGraph").is_some(),
        "摘除部分的结果要照样给出来"
    );
    assert!(
        out.payload.get("injected").is_none(),
        "没插就**不报** injected 键：{:?}",
        out.payload
    );
    assert!(
        out.content.contains("不敢"),
        "要如实说清为什么没插：{}",
        out.content
    );

    // ② ComfyUI 在（假实例的 /object_info 里没有 SageAttention）→ 也不硬插
    let history: &'static str = Box::leak(r#"{}"#.to_string().into_boxed_str());
    let view: &'static [u8] = Box::leak(Vec::new().into_boxed_slice());
    let info: &'static str = Box::leak(FIT_OBJECT_INFO.to_string().into_boxed_str());
    let (url, handle) = fake_comfy_full(history, view, info).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();
    let out = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": graph.clone(), "injections": inject }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["ok"], true);
    assert!(
        out.payload["injected"].as_array().unwrap().is_empty(),
        "实例里没有的类不许硬插"
    );
    assert!(
        out.payload["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("SageAttention")),
        "要如实说明少了什么：{:?}",
        out.payload["warnings"]
    );

    // ③ 实例里有这个类 → 真的插进去：采样器的 model 指向新节点，新节点的 model 接回 LoraLoader
    let with_sage: &'static str = Box::leak(
        r#"{"SageAttention": {"input":{"required":{"model":["MODEL"]}}}}"#
            .to_string()
            .into_boxed_str(),
    );
    let (url, handle2) = fake_comfy_full(history, view, with_sage).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();
    let out = call_tool(
        &state,
        "comfy_prune_workflow",
        serde_json::json!({ "graph": graph.clone(), "injections": inject }),
    )
    .await
    .unwrap();
    assert_eq!(
        out.payload["injected"].as_array().unwrap().len(),
        1,
        "{:?}",
        out.payload
    );
    let graph_out = &out.payload["graph"];
    let new_id = out.payload["injected"][0]
        .as_str()
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        graph_out["3"]["inputs"]["model"][0],
        serde_json::json!(new_id),
        "采样器挂到补丁上"
    );
    assert_eq!(graph_out[&new_id]["class_type"], "SageAttention");
    assert_eq!(
        graph_out[&new_id]["inputs"]["model"][0], "4",
        "补丁的 model 接回原来的上游"
    );
    assert_eq!(out.payload["runnable"], true);

    handle.abort();
    handle2.abort();
    std::fs::remove_dir_all(state.project_root()).ok();
}

// ===========================================================================
//  起 / 停实例（2026-09-28 用户 bug「通过 CLI 启动 comfyui 总是失败」）
//
//  这一对工具是"ComfyHub 自己拥有起实例这条路"的落点：COMfy-cli 那条路要写
//  `%LOCALAPPDATA%\comfy-cli\config.ini` 与 `<ComfyUI>\user\comfyui_8188.log`
//  两个**项目外**的文件，受限会话里必然 Errno 13（`comfy_launch.rs` 文件头有现场）。
//  端到端这一层盯的是两条**只能在这一层验**的纪律：
//   ① 已经有实例在跑时**不许**再起第二个（那只会撞端口 / 抢显存）；
//   ② 停的时候只认自己的 pid 记录 —— 用户自己的 ComfyUI Desktop 实例
//      **一个进程都不许碰**。
// ===========================================================================

#[tokio::test]
async fn start_and_stop_refuse_to_touch_what_is_not_ours() {
    let state = state();
    // 一个"已经在跑"的假实例（假服务对 /system_stats 也回 200 + JSON，够 comfy_start 判定）
    let (url, handle) = fake_comfy(r#"{}"#, &[]).await;
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: url.clone(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();

    let out = call_tool(&state, "comfy_start", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(out.payload["alreadyRunning"], true, "{}", out.payload);
    assert_eq!(out.payload["comfyReachable"], true);
    assert!(
        out.content.contains("已经在跑"),
        "要说清没再起一个：{}",
        out.content
    );

    // 停：没有 `.run\comfyui-<端口>.pid` → 如实拒绝（**绝不去猜一个 pid 杀**）
    let err = call_tool(&state, "comfy_stop", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "COMFY_NOT_OURS", "{}", err.message);
    assert!(
        err.message.contains("不是 ComfyHub 起的"),
        "{}",
        err.message
    );
    handle.abort();
}
