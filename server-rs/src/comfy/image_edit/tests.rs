//! `image_edit` 的单元测试（主体代码在 [`image_edit`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 投放文件名可重复且不互相覆盖() {
    // 同一个来源 → 同一个名字（重跑工作流不会指向一个已经没有的文件）
    assert_eq!(
        input_filename_for("image.png", "att_7f3a9c21", None),
        input_filename_for("image.png", "att_7f3a9c21", None)
    );
    // 不同来源、同一个原文件名 → 不互相覆盖
    assert_ne!(
        input_filename_for("image.png", "att_aaaabbbb", None),
        input_filename_for("image.png", "att_ccccdddd", None)
    );
    // 路径分隔符与奇怪字符一律清掉（ComfyUI 只认裸文件名）——
    // ⚠ 这里与 Kotlin **逐字一致**：它把整串都做字符映射（不先取 basename），
    //   所以结果里会留下一串下划线开头。**没有穿越风险**：分隔符已经全变成 `_`，
    //   再 join 到 input 目录之下也拼不出第二个目录。
    assert_eq!(
        input_filename_for(r"..\..\etc\pass wd.PNG", "m310", None),
        "_.._etc_pass_wd-m310.PNG"
    );
    // 全是点 → 清完之后空掉 → 回落到 image.png（否则会拼出一个没有名字的文件）
    assert_eq!(input_filename_for("....", "m310", None), "image-m310.png");
    // 显式文件名优先（id 后缀插在**扩展名之前**，所以 LoadImage 还认得出它是 png）
    assert_eq!(
        input_filename_for("orig.png", "m310", Some("bg.png")),
        "bg-m310.png"
    );
    // 没有任何扩展名时补 .png（ComfyUI 的 LoadImage 按扩展名找解码器）
    assert_eq!(input_filename_for("noext", "m1", None), "noext-m1.png");
    // 纯扩展名（`.png`）要当"没有扩展名"处理，别把 stem 变成空串
    assert_eq!(input_filename_for(".png", "m1", None), "png-m1.png");
}

#[test]
fn region_预设与显式坐标都对() {
    assert_eq!(parse_region("full").unwrap(), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(parse_region("PERSON").unwrap(), [0.22, 0.05, 0.56, 0.92]);
    assert_eq!(parse_region("").unwrap(), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(parse_region("0.5,0,0.5,1").unwrap(), [0.5, 0.0, 0.5, 1.0]);
    // 越界的宽高要**夹到边界内**（0.8+0.5 > 1），而不是画到画布外面去
    assert_eq!(
        parse_region("0.8,0.8,0.5,0.5").unwrap(),
        [0.8, 0.8, 0.19999999999999996, 0.19999999999999996]
    );
}

#[test]
fn region_四种拒绝都是人话() {
    let too_few = parse_region("0.1,0.2").unwrap_err();
    assert_eq!(too_few.code, "INVALID_ARGUMENT");
    assert!(
        too_few.message.contains("x,y,w,h"),
        "要说清该给什么：{}",
        too_few.message
    );
    assert!(
        parse_region("0.1,0.2,0.3,第四").is_err(),
        "非数字要报错，不许静默当 0"
    );
    assert!(
        parse_region("1.2,0,0.5,0.5").is_err(),
        "超过 1 要报错（它是归一化的）"
    );
    assert!(
        parse_region("1,0,0,0.5").is_err(),
        "宽高为 0 要报错（否则遮罩是全黑）"
    );
}

#[test]
fn 矩形遮罩_白区的位置与尺寸都对() {
    // 40x40 的画布上取中间那一半：白区必须**恰好在** [4,4)~(20,20)
    let region = [0.1, 0.1, 0.4, 0.4];
    let pixels = draw_mask(40, 40, &region, "rect");
    let at = |x: usize, y: usize| pixels[y * 40 + x];
    assert_eq!(at(0, 0), 0, "左上角是黑（保留）");
    assert_eq!(at(4, 4), 255, "白区左上角");
    assert_eq!(at(19, 19), 255, "白区右下角（含）");
    assert_eq!(at(20, 20), 0, "白区之外一个像素就是黑");
    assert_eq!(at(39, 39), 0);
    assert_eq!(pixels.iter().filter(|v| **v == 255).count(), 16 * 16);
}

#[test]
fn 椭圆遮罩_边缘是抗锯齿的灰() {
    let pixels = draw_mask(64, 64, &[0.0, 0.0, 1.0, 1.0], "ellipse");
    let center = pixels[32 * 64 + 32];
    assert_eq!(center, 255, "中心是纯白");
    assert_eq!(pixels[0], 0, "四角在椭圆之外 → 黑");
    // 边缘必须存在中间值（全 0/255 就说明没做抗锯齿，贴人物时会出现硬台阶）
    assert!(
        pixels.iter().any(|v| *v > 0 && *v < 255),
        "椭圆边缘应该有抗锯齿的灰阶"
    );
}

#[test]
fn 反选把里外翻过来() {
    let normal = draw_mask(32, 32, &[0.25, 0.25, 0.5, 0.5], "rect");
    let inverted: Vec<u8> = normal.iter().map(|v| 255 - *v).collect();
    assert_eq!(normal[16 * 32 + 16], 255);
    assert_eq!(inverted[16 * 32 + 16], 0);
    assert_eq!(normal[0], 0);
    assert_eq!(inverted[0], 255);
}

#[test]
fn 羽化_只糊边缘不动中心() {
    // 白区右边一半，羽化 16 → 半径 8 的均值模糊
    let region = [0.5, 0.0, 0.5, 1.0];
    let sharp = draw_mask(64, 64, &region, "rect");
    let blurred = box_blur(&sharp, 64, 64, 16);
    let at = |p: &Vec<u8>, x: usize, y: usize| p[y * 64 + x];
    assert_eq!(at(&sharp, 32, 32), 255, "硬边界：第 32 列是白的");
    assert!(
        at(&blurred, 32, 32) < 255 && at(&blurred, 32, 32) > 0,
        "羽化后边界变成过渡值"
    );
    assert_eq!(at(&blurred, 63, 32), 255, "远离边界的地方仍是纯白");
    assert_eq!(at(&blurred, 0, 32), 0, "远离边界的地方仍是纯黑");
    // 羽化**不该**让整张图变暗（边缘钳位保证这一点）
    assert_eq!(at(&blurred, 63, 0), 255);
}

#[test]
fn 指纹只由参数决定() {
    let a = content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 16);
    let b = content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 16);
    assert_eq!(a, b, "同一组参数必须得到同一个文件名");
    assert_eq!(a.len(), 12, "6 个字节的十六进制");
    assert_ne!(
        a,
        content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", true, 16)
    );
    assert_ne!(
        a,
        content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 24)
    );
    assert_ne!(
        a,
        content_tag(768, 1024, &[0.5, 0.26, 0.36, 0.74], "rect", false, 16)
    );
}

#[test]
fn 取值容错但不猜() {
    let args = json!({"w": "768", "h": 1024.0, "b": "true", "n": "yes", "s": "  bg.png  "});
    assert_eq!(int_arg(&args, "w"), Some(768));
    assert_eq!(int_arg(&args, "h"), Some(1024));
    assert_eq!(int_arg(&args, "missing"), None);
    assert_eq!(bool_arg(&args, "b"), Some(true));
    assert_eq!(bool_arg(&args, "n"), None, "「yes」不许猜成 true");
    assert_eq!(str_arg(&args, "s"), Some("bg.png".to_string()));
    assert_eq!(str_arg(&args, "empty"), None);
}

/// 造一张"白底 + 一个红方块"的图（模拟阶段一的白底产物）。
fn white_bg_with_block(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> image::RgbImage {
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([252, 252, 250]));
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, image::Rgb([200, 30, 40]));
        }
    }
    img
}

/// 一个"人物图已经投进 input、但 input 目录**写不进**"的后端状态。
///
/// ⚠ **造法必须与受限现场同错码，且不依赖这台机器的目录 ACL**：
/// 在目标文件名上**放一个同名目录** —— Windows 上"在目录的位置建文件"必然回
/// `ERROR_ACCESS_DENIED (os error 5)`，正是"映像在工作区内"那一类进程拿到的错。
/// （`crate::infra::confinement` 的 tests 用的是同一个手法，两处都别改成"改 ACL"。）
fn 写不进_input_的状态(label: &str) -> (crate::web::routes::AppState, PathBuf) {
    let root = crate::infra::testpaths::scratch(&format!("image-edit-{label}"));
    let shared = root.join("shared");
    let input = shared.join("input");
    let output = shared.join("output");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    white_bg_with_block(200, 200, 70, 40, 130, 160)
        .save(input.join("person-m344.png"))
        .unwrap();

    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .expect("读得到真 schema");
    conn.execute_batch(&schema).expect("建 schema");
    let state = crate::web::routes::AppState::new(conn, ":memory:", root.join("media"));
    crate::comfy::capture::save_capture_config(
        &state,
        crate::comfy::capture::CaptureConfig {
            output_dir: Some(output.display().to_string()),
            ..Default::default()
        },
    )
    .expect("存捕获配置");
    (state, input)
}

/// **摆放后的人物图直写被拒时，要改让 ComfyUI 自己写，而不是当场报错**
/// （现象：阶段二在"写入摆好的人物图失败：拒绝访问 (os error 5)"处中断，
/// 于是没拿到 `personFilename`、也提交不了合成；同一轮里 `comfy_use_media` 却成功了
/// ——因为它走的是 `/upload/image`）。
#[test]
fn 摆放的人物图写不进时改走_upload_而不是报错() {
    let (_state, input) = 写不进_input_的状态("placed");
    // 占住"摆好的人物图"该落地的那个文件名 → 直写必然拿 ERROR_ACCESS_DENIED
    std::fs::create_dir(input.join("placed-person-m344.png")).unwrap();

    let args = json!({
        "mode": "subject",
        "subjectFilename": "person-m344.png",
        "width": 200,
        "height": 200,
        "scale": 1.0,
        "x": 0.5,
        "y": 0.5,
    });
    let mut delivery = Delivery::for_input_dir(&input);
    let result = make_mask_with(&args, &input, &mut delivery)
        .expect("直写被拒也要给出结果（那份内容攒给 ComfyUI 写），不能当场报错");

    assert!(
        !delivery.is_empty(),
        "写不进去的那一份必须攒着等 /upload/image"
    );
    let (target, bytes, why) = &delivery.pending[0];
    assert_eq!(
        target.file_name().unwrap().to_string_lossy(),
        "placed-person-m344.png",
        "攒的必须是**摆好的那一份**（丢错文件 = 阶段二拿错图）"
    );
    assert!(
        bytes.starts_with(&[0x89, b'P', b'N', b'G']),
        "攒的要是编好的 PNG 字节：{:02x?}",
        &bytes[..4]
    );
    assert!(
        why.contains("os error 5"),
        "直写被拒的原因要原样带出来：{why}"
    );

    assert_eq!(result["personFilename"], "placed-person-m344.png");
    assert_eq!(result["placedSubjectFilename"], "placed-person-m344.png");
    assert_eq!(result["maskGenerated"], false, "主体档本来就不写遮罩");
    assert!(
        result.get("channel").is_none(),
        "还没 flush，不许先说走了 upload"
    );
    crate::infra::testpaths::cleanup(input.parent().unwrap().parent().unwrap());
}

/// **ComfyUI 认下的名字才是 `LoadImage` 认得的那个** —— flush 完必须按名字回填，
/// 否则模型会拿一个"我们以为写进去了"的名字去覆盖工作流（表现是 `Invalid image file`）。
#[test]
fn 上传回来的文件名会回填到结果里() {
    let mut result = json!({
        "filename": "comfyhub_mask_ab12cd.png",
        "personFilename": "placed-person-m344.png",
        "placedSubjectFilename": "placed-person-m344.png",
    });
    apply_uploaded_names(
        &mut result,
        &[
            (
                "placed-person-m344.png".to_string(),
                "placed-person.png".to_string(),
            ),
            (
                "comfyhub_mask_ab12cd.png".to_string(),
                "comfyhub_mask_ab12cd.png".to_string(),
            ),
        ],
    );
    assert_eq!(result["personFilename"], "placed-person.png");
    assert_eq!(result["placedSubjectFilename"], "placed-person.png");
    assert_eq!(result["filename"], "comfyhub_mask_ab12cd.png");
    assert_eq!(result["channel"], "comfy-upload");

    // 什么都没上传（直写就成功了）→ 一个键都不许动
    let mut untouched = json!({"personFilename": "person-m344.png"});
    apply_uploaded_names(&mut untouched, &[]);
    assert_eq!(untouched, json!({"personFilename": "person-m344.png"}));
}

/// 遮罩那一档**走的是同一条退路**（它早就有，但没人钉住"被拒时不报错"）。
#[test]
fn 遮罩写不进时也改走_upload() {
    let (_state, input) = 写不进_input_的状态("mask");
    std::fs::create_dir(input.join("blocked-mask.png")).unwrap();
    let args = json!({
        "mode": "rect",
        "region": "0.1,0.1,0.4,0.4",
        "width": 120,
        "height": 120,
        "filename": "blocked-mask.png",
    });
    let mut delivery = Delivery::for_input_dir(&input);
    let result = make_mask_with(&args, &input, &mut delivery).expect("直写被拒也不当场报错");
    assert_eq!(result["maskGenerated"], true);
    assert_eq!(delivery.pending.len(), 1);
    assert_eq!(
        delivery.pending[0].0.file_name().unwrap().to_string_lossy(),
        "blocked-mask.png"
    );
    crate::infra::testpaths::cleanup(input.parent().unwrap().parent().unwrap());
}

/// 写得进去时就**别多走一趟 HTTP**（投放原图每次都上传会白白多花几十 MB 流量）。
#[test]
fn 写得进去时不攒投递() {
    let (_state, input) = 写不进_input_的状态("direct");
    let args = json!({
        "mode": "rect",
        "region": "full",
        "width": 120,
        "height": 120,
        "filename": "plain-mask.png",
    });
    let mut delivery = Delivery::for_input_dir(&input);
    make_mask_with(&args, &input, &mut delivery).unwrap();
    assert!(
        delivery.is_empty(),
        "直写成功就不该攒：{}",
        input.join("plain-mask.png").display()
    );
    assert!(
        input.join("plain-mask.png").is_file(),
        "而且文件真的要落盘了"
    );
    crate::infra::testpaths::cleanup(input.parent().unwrap().parent().unwrap());
}

/// `mode="subject"`：白底上抠一个方块，遮罩就该是**那个方块的形状**（不是整块矩形画面）。
///
/// 这是"贴人物图出现一圈白框"的解法：
/// `ImageCompositeMasked` 按遮罩搬像素，遮罩是矩形就会把人物图那块矩形连背景一起搬进去。
#[test]
fn 主体遮罩抠出的是形状不是整块矩形() {
    let dir = crate::infra::testpaths::scratch("subject");
    let path = dir.join("person.png");
    // 400x400 的白底，中间一个 100x200 的红方块（模拟站着的人）
    white_bg_with_block(400, 400, 150, 100, 250, 300)
        .save(&path)
        .unwrap();

    let (mask, background, ratio) = subject_mask(
        &path,
        400,
        400,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    assert_eq!(background, [252, 252, 250], "背景色要从四周一圈估出来");
    let at = |x: usize, y: usize| mask[y * 400 + x];
    assert_eq!(at(200, 200), 255, "方块里是白的（要重绘）");
    assert_eq!(at(20, 20), 0, "白底是黑的（保留）—— 这一条就是白框的解药");
    assert_eq!(at(390, 390), 0);
    // 占比 ≈ 100*200/400*400 = 12.5%
    assert!(
        (ratio - 0.125).abs() < 0.02,
        "占比应当接近 12.5%，实测 {ratio}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn 主体遮罩只留最大的一块连通区并支持与矩形取交集() {
    let dir = crate::infra::testpaths::scratch("subject2");
    let path = dir.join("person.png");
    let mut img = white_bg_with_block(300, 300, 120, 60, 180, 260);
    // 角落里一个孤立的小红块：背景噪点 / 水印纹的替身 —— 必须被丢掉
    for y in 10..20 {
        for x in 10..20 {
            img.put_pixel(x, y, image::Rgb([210, 40, 40]));
        }
    }
    img.save(&path).unwrap();

    let (mask, _, _) = subject_mask(
        &path,
        300,
        300,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    assert_eq!(mask[15 * 300 + 15], 0, "孤立的小块要当噪点丢掉");
    assert_eq!(mask[150 * 300 + 150], 255);

    // 与 region 取交集：方块横跨 x=120..180，画布 300 → 中线在 150
    let (left, _, _) = subject_mask(
        &path,
        300,
        300,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 0.5, 1.0],
        true,
        None,
    )
    .unwrap();
    assert_eq!(left[150 * 300 + 140], 255, "左半边里、方块内的点要留着");
    assert_eq!(left[150 * 300 + 160], 0, "region 之外必须清零");
    let (right, _, _) = subject_mask(
        &path,
        300,
        300,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.5, 0.0, 0.5, 1.0],
        true,
        None,
    )
    .unwrap();
    assert_eq!(right[150 * 300 + 160], 255, "右半边里、方块内的点要留着");
    assert_eq!(right[150 * 300 + 130], 0, "region 之外必须清零");
    std::fs::remove_dir_all(&dir).ok();
}

/// 尺寸不一致时遮罩要**按块平均缩放到画布**（与工作流里 `ImageScale` 的口径一致），
/// 否则剪影与缩放后的像素对不上，贴出来就是错位的。
#[test]
fn 主体遮罩会缩放到画布尺寸() {
    let dir = crate::infra::testpaths::scratch("subject3");
    let path = dir.join("person.png");
    white_bg_with_block(400, 400, 150, 100, 250, 300)
        .save(&path)
        .unwrap();

    let (mask, _, ratio) = subject_mask(
        &path,
        200,
        200,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    assert_eq!(mask.len(), 200 * 200);
    assert_eq!(mask[100 * 200 + 100], 255, "缩放后方块中心仍是白的");
    assert_eq!(mask[10 * 200 + 10], 0);
    assert!((ratio - 0.125).abs() < 0.03, "缩放后占比不变：{ratio}");
    std::fs::remove_dir_all(&dir).ok();
}

/// **主体自己也与背景同色**时（白裙子 / 白衬衫 / 高光）要能填回来 ——
/// 2026-09-26 现场：白底人物图贴进夜景后"人变成半透明"（裙子那块透出街景）。
/// 判据只能是连通性：被深色描边围住的白色窟窿属于主体。
#[test]
fn 主体遮罩要把被包住的同色窟窿填回来() {
    let dir = crate::infra::testpaths::scratch("subject4");
    let path = dir.join("person.png");
    // 白底 + 一个**深色描边的空心方块**（方块内部也是白的 = 白裙子）
    let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([252, 252, 250]));
    for y in 40..160 {
        for x in 60..140 {
            let edge = !(44..156).contains(&y) || !(64..136).contains(&x);
            img.put_pixel(
                x,
                y,
                if edge {
                    image::Rgb([20, 20, 24])
                } else {
                    image::Rgb([250, 250, 248])
                },
            );
        }
    }
    img.save(&path).unwrap();

    let (mask, _, ratio) = subject_mask(
        &path,
        200,
        200,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    assert_eq!(
        mask[100 * 200 + 100],
        255,
        "被描边围住的白色内部必须算主体（否则人变半透明）"
    );
    assert_eq!(mask[10 * 200 + 10], 0, "画面四角的真背景不许被填");
    // 实心方块 80x120 ≈ 24%（填洞前后都是这个数）
    assert!(
        (ratio - 0.24).abs() < 0.03,
        "占比应当接近 24%，实测 {ratio}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **绿幕背景**：主体不含背景色 → **不填内部空洞**，腿间/发丝之间的背景老实留着。
///
/// 正解："应该用不同于画面主体颜色的颜色作为背景" ——
/// 白底 + 白裙子在像素上无法区分，绿幕就完全没有这个二义性。
#[test]
fn 绿幕背景不填洞且腿间留空() {
    let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([0, 177, 64]));
    // 主体：两条"腿" + 上面一条"胯"把它们连成**一块**（模拟人形），
    // 腿间保留一条绿缝（即看到的那圈白 —— 它必须留在背景内）
    for y in 40..60 {
        for x in 70..130 {
            img.put_pixel(x, y, image::Rgb([30, 30, 40]));
        }
    }
    for y in 60..160 {
        for x in 70..95 {
            img.put_pixel(x, y, image::Rgb([30, 30, 40]));
        }
        for x in 105..130 {
            img.put_pixel(x, y, image::Rgb([30, 30, 40]));
        }
    }
    let (mask, background, _) = subject_mask_from_image(
        &img,
        200,
        200,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    assert_eq!(background, [0, 177, 64], "绿幕要能从边框认出来");
    let saturation = background.iter().max().unwrap() - background.iter().min().unwrap();
    assert!(
        saturation as i32 >= CHROMA_MIN_SATURATION,
        "绿幕是高饱和 → 走不填洞那一档"
    );
    assert_eq!(
        mask[100 * 200 + 100],
        0,
        "腿间那条绿缝必须留在背景里（这就是用户看到的那圈白）"
    );
    assert_eq!(mask[100 * 200 + 80], 255, "左边那条腿是主体");
    assert_eq!(mask[100 * 200 + 115], 255, "右边那条腿是主体");
    assert_eq!(mask[5 * 200 + 5], 0);
}

/// 白底 + 白裙子：仍然要填洞（否则人变半透明），但这正是**二义**所在 ——
/// 所以返回值里会带 `chromaAdvice` 提醒改用绿幕。
#[test]
fn 白底仍然填洞但会提醒改绿幕() {
    let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([252, 252, 250]));
    for y in 40..160 {
        for x in 70..130 {
            let edge = !(44..156).contains(&y) || !(74..126).contains(&x);
            img.put_pixel(
                x,
                y,
                if edge {
                    image::Rgb([20, 20, 24])
                } else {
                    image::Rgb([250, 250, 248])
                },
            );
        }
    }
    let (mask, background, _) = subject_mask_from_image(
        &img,
        200,
        200,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    let saturation = background.iter().max().unwrap() - background.iter().min().unwrap();
    assert!(
        (saturation as i32) < CHROMA_MIN_SATURATION,
        "白底是低饱和 → 走填洞那一档"
    );
    assert_eq!(mask[100 * 200 + 100], 255, "被描边围住的白色内部要填回来");
}

/// `bgColor`：显式给就用给的（不再去猜边框），认不出来的写法**不猜**。
#[test]
fn 背景色解析只认确定写法() {
    assert_eq!(parse_color("#00b140"), Some([0, 177, 64]));
    assert_eq!(parse_color("green"), Some([0, 177, 64]));
    assert_eq!(parse_color("绿幕"), Some([0, 177, 64]));
    assert_eq!(parse_color("magenta"), Some([255, 0, 255]));
    assert_eq!(parse_color("#0b0"), Some([0, 187, 0]));
    assert_eq!(parse_color("white"), Some([255, 255, 255]));
    assert_eq!(
        parse_color("浅蓝"),
        None,
        "认不出来就回落成「从边框估」，不猜"
    );
}
/// **细发丝不得被删除**（现象："细的头发也被去掉"）：
/// 与主体只隔着几个半透明像素的细条，必须靠"膨胀后取最大连通区再取交集"保住。
#[test]
fn 与主体细连接的细发丝不会被当噪点删掉() {
    let (w, h) = (80usize, 80usize);
    let mut mask = vec![0u8; w * h];
    // 主体：40x40 的方块
    for y in 20..60 {
        for x in 20..60 {
            mask[y * w + x] = 255;
        }
    }
    // 细化丝：从主体右侧伸出一条 1 像素宽的线，中间**断开 2 像素**（模拟被半透明像素隔开）
    for x in 60..72 {
        mask[40 * w + x] = 255;
    }
    for x in 74..80 {
        mask[40 * w + x] = 255;
    }
    keep_largest_blob(&mut mask, w, h);
    assert_eq!(mask[40 * w + 65], 255, "连着主体的发丝要保住");
    assert_eq!(
        mask[40 * w + 76],
        255,
        "断口只有 2 像素、膨胀 3 能连上 → 也要保住"
    );
    // 真正孤立的远处碎块（离主体 > 3 像素）照旧删掉
    for y in 2..5 {
        for x in 2..5 {
            mask[y * w + x] = 255;
        }
    }
    keep_largest_blob(&mut mask, w, h);
    assert_eq!(mask[3 * w + 3], 0, "孤立的噪点还是要删");
}

/// **摆放**：缩放到一半、移到左下角 ——
/// 遮罩与人物像素必须落在**同一个**位置（只动一个就是「人在这儿、遮罩在那儿」的错位）。
/// **已删除那一档的老参数必须被明确拒绝**（不许静默忽略）。
///
/// 为什么这条要单独钉住：模型是照**工具描述**写参数的，而"描述已经改了、模型手里还是旧
/// 记忆"是常态。静默忽略的后果不是报错，是**语义错位** —— 它以为 `matteSource="model"`
/// 意思是"遮罩由你写"，实际拿到的是"不写遮罩"，于是去覆盖一个不存在的 `LoadImageMask`。
#[test]
fn 已删除的颜色档参数会被明确拒绝() {
    let args = json!({
        "baseFilename": "x.png",
        "mode": "subject",
        "subjectFilename": "person.png",
        "matteSource": "model",
    });
    let err = reject_removed_args(&args).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(
        err.message.contains("matteSource"),
        "要指名道姓说是哪个参数：{}",
        err.message
    );
    assert!(
        err.message.contains("已删除"),
        "要说清是删掉了、不是写错了：{}",
        err.message
    );

    // 每一个老参数都要被认出来（漏一个 = 它会被当成未知键悄悄忽略）
    for removed in REMOVED_ARGS {
        let mut map = serde_json::Map::new();
        map.insert((*removed).to_string(), json!(1));
        map.insert("baseFilename".to_string(), json!("x.png"));
        map.insert("mode".to_string(), json!("rect"));
        map.insert("region".to_string(), json!("0.1,0.1,0.2,0.2"));
        let err = reject_removed_args(&Value::Object(map)).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{removed} 应当被拒");
        assert!(
            err.message.contains(removed),
            "{removed} 的报错要点名它：{}",
            err.message
        );
    }

    // 正常参数一个都不许误伤
    let ok = json!({
        "baseFilename": "x.png",
        "mode": "subject",
        "subjectFilename": "person.png",
        "bgColor": "green",
        "scale": 1.0,
        "x": 0.5,
        "y": 0.5,
    });
    assert!(reject_removed_args(&ok).is_ok());
}

#[test]
fn 摆放要缩放并搬到位且图与遮罩不脱节() {
    let mut img = image::RgbImage::from_pixel(400, 400, image::Rgb([250, 250, 250]));
    for y in 100..300 {
        for x in 150..250 {
            img.put_pixel(x, y, image::Rgb([200, 30, 40]));
        }
    }
    let (mask, _, _) = subject_mask_from_image(
        &img,
        400,
        400,
        SUBJECT_TOLERANCE_DEFAULT,
        &[0.0, 0.0, 1.0, 1.0],
        false,
        None,
    )
    .unwrap();
    let bbox = mask_bbox(&mask, 400, 400).unwrap();
    assert_eq!(bbox, (150, 100, 250, 300));

    let (placed_img, placed_mask) = place_patch(&img, &mask, 400, 400, bbox, 0.5, 0.25, 0.75);
    let new_box = mask_bbox(&placed_mask, 400, 400).unwrap();
    let (cx, cy) = ((new_box.0 + new_box.2) / 2, (new_box.1 + new_box.3) / 2);
    assert_eq!((cx, cy), (100, 300), "中心要搬到 (0.25,0.75) x 400");
    assert_eq!(
        (new_box.2 - new_box.0, new_box.3 - new_box.1),
        (50, 100),
        "宽高各缩一半"
    );

    // 图与遮罩不脱节
    let pixel = *placed_img.get_pixel(cx as u32, cy as u32);
    assert!(
        pixel[0] > 150 && pixel[1] < 80,
        "新位置上的像素应当是人物（红），实测 {pixel:?}"
    );
    assert_eq!(placed_mask[cy * 400 + cx], 255);
    assert_eq!(
        placed_mask[200 * 400 + 200],
        0,
        "原位置必须清掉（否则老地方留个鬼影）"
    );
    let old = *placed_img.get_pixel(200, 200);
    assert!(
        old[0] < 40 && old[1] < 40,
        "原位置的像素也要清掉，实测 {old:?}"
    );
}

#[test]
fn 只取文件名挡目录穿越() {
    assert_eq!(file_name_only(r"..\..\windows\system32\x.png"), "x.png");
    assert_eq!(file_name_only("/etc/passwd"), "passwd");
    assert_eq!(file_name_only("  bg.png  "), "bg.png");
}

/// 两处都没有 → **如实报 `COMFY_NOT_FOUND`**（这一支在开发机上走不到：
/// `find_home` 能探测到本机 ComfyUI，而它正是首次使用时遇到的分支）。
#[test]
fn 找不到_comfyui_时如实报错() {
    let failure = input_dir_from(Vec::new()).unwrap_err();
    assert_eq!(failure.code, "COMFY_NOT_FOUND");
    assert!(
        failure.message.contains("设置"),
        "要告诉用户去哪儿确认：{}",
        failure.message
    );
}

/// 候选顺序：**先到先得**，第一个真实存在的目录就是投放目标。
///
/// 回归用例：发布包的 `capture.config` 中**没有 outputDir**，
/// 于是只能退到"DeskTop 配的共享 input"（我们起实例时挂的就是它）—— 不能退到
/// `find_home` 探到的**安装目录**那份（实例根本不读它，提交必 400 `Invalid image file`）。
#[test]
fn 候选目录按顺序取第一个存在的() {
    let root = crate::infra::testpaths::scratch("input-dirs");
    let configured = root.join("shared");
    let probed = root.join("probed");
    for dir in [configured.join("input"), probed.join("input")] {
        std::fs::create_dir_all(dir).unwrap();
    }

    // ① 配置的输出目录排第一：配置里已经指明了，就不该用后面那些
    let picks = input_dir_from(vec![
        (configured.join("input"), "capture-output"),
        (probed.join("input"), "probe"),
    ])
    .unwrap();
    assert_eq!(picks, configured.join("input"));

    // ② 配置中的那份**不存在**时跳过，取下一个实际存在的（首次使用 / 更换机器的情形）
    let picks = input_dir_from(vec![
        (root.join("nope").join("input"), "capture-output"),
        (probed.join("input"), "desktop-config"),
    ])
    .unwrap();
    assert_eq!(picks, probed.join("input"), "不存在的候选不许占位");

    // ③ 一个都没有 → 如实报错（不建目录、不猜）
    assert_eq!(
        input_dir_from(Vec::new()).unwrap_err().code,
        "COMFY_NOT_FOUND"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// **实例答话就是终点**：哪怕我们按配置推出来的是另一个目录，也得听实例的。
///
/// 该问题的判据本身 —— 情形为"图投进 A、实例读取 B"，
/// 提交回 400 `Invalid image file`。所以：
///  * 不一致 → 使用实例那份，**并把两个目录都写入 note**（界面与模型均需可见）；
///  * 一致 → 用实例那份，但**不加 note**（一致时说一句"其实是一样的"是噪音）。
#[test]
fn 实例说的_input_覆盖我们按配置推的那份() {
    let root = crate::infra::testpaths::scratch("choose-input");
    let install = root.join("ComfyUI-Installs").join("ComfyUI").join("input");
    let shared = root.join("ComfyUI-Shared").join("input");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    let candidates = vec![
        (install.clone(), "capture-output"),
        (shared.clone(), "desktop-config"),
    ];

    // ① 实例读的是**共享**那份，而配置推出来的是**安装目录**那份 → 以实例为准 + 说清两边
    let picked = choose_input_dir(&candidates, Some(shared.clone())).unwrap();
    assert_eq!(picked.dir, shared, "写错那一份的表现就是提交时 400");
    assert_eq!(picked.source, "instance");
    let note = picked.note.expect("不一致时必须如实说出来");
    assert!(
        note.contains(&install.display().to_string()),
        "两个目录都要点名：{note}"
    );
    assert!(note.contains(&shared.display().to_string()), "{note}");

    // ② 两边一致 → 同样以实例为准，但没有 note
    let picked = choose_input_dir(&candidates, Some(install.clone())).unwrap();
    assert_eq!(picked.dir, install);
    assert_eq!(picked.source, "instance");
    assert!(picked.note.is_none(), "一致时别加噪音：{:?}", picked.note);

    // ③ 实例不答话（没在跑 / 不是 ComfyUI）→ 回落候选顺序，source 如实标它的来路
    let picked = choose_input_dir(&candidates, None).unwrap();
    assert_eq!(picked.dir, install);
    assert_eq!(picked.source, "capture-output");
    assert!(picked.note.is_none());

    // ④ 候选全不存在 + 实例不答话 → 如实报错（不建目录、不猜）
    let missing = vec![(root.join("nope").join("input"), "capture-output")];
    assert_eq!(
        choose_input_dir(&missing, None).unwrap_err().code,
        "COMFY_NOT_FOUND"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// 配置的 outputDir → 它的兄弟 `input`；配的是 input 本身 / 名字不叫 output 时**原样用**。
#[test]
fn 配置的输出目录推兄弟_input() {
    let root = crate::infra::testpaths::scratch("input-sibling");
    assert_eq!(
        sibling_input_of(Some(
            &root
                .join("ComfyUI-Shared")
                .join("output")
                .display()
                .to_string()
        )),
        Some(root.join("ComfyUI-Shared").join("input")),
        "output 的兄弟目录"
    );
    // 直接填写了 input 目录（旧设置页允许）→ 原样使用，不再追加一层
    assert_eq!(
        sibling_input_of(Some(
            &root.join("shared").join("input").display().to_string()
        )),
        Some(root.join("shared").join("input"))
    );
    // 大小写不敏感：Windows 上 `Output` 也是那个 output
    assert_eq!(
        sibling_input_of(Some(
            &root.join("shared").join("Output").display().to_string()
        )),
        Some(root.join("shared").join("input"))
    );
    assert_eq!(sibling_input_of(None), None);
    assert_eq!(sibling_input_of(Some("   ")), None);
    std::fs::remove_dir_all(&root).ok();
}

/// 文件不在它会自己去 HuggingFace **静默下载**（表现是"点了按钮没反应"）——
/// 所以这个检查要如实，而且要把"**每个**根下面各有什么"都列出来
/// （真机上"装了模型的那个根"与"配置的输出目录那个根"是两处，只查一处就会误报"缺"）。
#[test]
fn 抠图模型检查只认_comfyui_根下的_rembg_且如实报缺() {
    let base = crate::infra::testpaths::scratch("matte-models");
    let with_model = base.join("install");
    let without = base.join("shared");
    let not_comfy = base.join("storage");
    // ① 有模型的根：`output/` + `models/` 就够"看起来是 ComfyUI"，再放一个 BEN2
    std::fs::create_dir_all(with_model.join("output")).unwrap();
    std::fs::create_dir_all(with_model.join("models").join("rembg")).unwrap();
    std::fs::write(
        with_model
            .join("models")
            .join("rembg")
            .join("BEN2_Base.pth"),
        b"x",
    )
    .unwrap();
    // ② 同名的空目录（**本机 Shared 那份 rembg 就是空的** —— 只查它就会误报"缺"）
    std::fs::create_dir_all(without.join("output")).unwrap();
    std::fs::create_dir_all(without.join("models").join("rembg")).unwrap();
    // ③ 根本不像 ComfyUI 的目录要跳过（候选里还混着我们自己的目录）
    std::fs::create_dir_all(&not_comfy).unwrap();

    let out = matting_models(&[with_model.clone(), without.clone(), not_comfy.clone()]);
    assert_eq!(out["usable"], true);
    let checked = out["checked"].as_array().unwrap();
    assert_eq!(
        checked.len(),
        2,
        "只该列真像 ComfyUI 的那两个根：{checked:?}"
    );
    assert!(
        checked[0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "BEN2_Base.pth"),
        "有模型的那份要报出来：{checked:?}"
    );
    assert!(
        checked[1]["files"].as_array().unwrap().is_empty(),
        "空的那份要如实报空"
    );
    assert!(out["download"]["RMBG-1.4.pth"]
        .as_str()
        .unwrap()
        .contains("RMBG-1.4.pth"));
    assert!(
        out["note"].as_str().unwrap().contains("静默下载"),
        "要说清「别硬跑」的原因（那个节点会自己去下）"
    );

    // 一个都没有 → usable=false（调用方据此提示下载，而不是强行执行）
    let none = matting_models(std::slice::from_ref(&without));
    assert_eq!(none["usable"], false);
    assert_eq!(none["checked"].as_array().unwrap().len(), 1);

    std::fs::remove_dir_all(&base).ok();
}
