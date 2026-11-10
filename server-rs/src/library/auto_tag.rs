//! 自动标签：从**提示词正文**与**模型信息**推断人看得懂的标签。
//!
//! 入库与「给已有内容补标签」共用这一层；全部是**本地确定性规则**，不联网、不调模型：
//!
//! | 来源 | 例子 | 分类 |
//! | --- | --- | --- |
//! | 提示词正文 | `1girl, ink painting` → `少女`、`水墨` | 风格 / 角色 / 场景 / 画质 / 构图 / 氛围 |
//! | 底模文件名 | `anima-base-v1.0.safetensors` → `Anima` | 模型 |
//! | LoRA 文件名 | `add_detail.safetensors` → `add detail` | LoRA |
//! | 产物类型 | 视频 → `视频` | 类型 |
//! | NSFW 标记 | 手动或 AI 标记过 → `NSFW` | 内容 |
//!
//! # 四条纪律
//!
//! 1. **纯函数、可单测**：推断只看传入的字段，不读库、不看环境变量。入库与「给存量补标签」
//!    走的是同一个 [`infer`] —— 两条路的标签集合必须一致。
//! 2. **只加不减**：自动标签永远只做追加（`INSERT OR IGNORE`），绝不删除已有标签。
//!    手动删掉的自动标签，下次点「补标签」会回来；这是"补"的语义，不是覆盖。
//! 3. **宁可少打，不许乱打**：英文词按词边界匹配（`cat` 不会命中 `location`），
//!    推断不出模型家族时用清理过的文件名，而不是把整条路径塞进标签。
//! 4. **分类是给人筛的**：分类字符串直接进 `tags.category`，界面上的分类筛选就按它分组。

use std::collections::HashSet;

use crate::library::repo;
use crate::web::routes::{ApiError, AppState};
use crate::workflow::graph_parse::{LoraRef, Parsed};

/// 一条推断出来的标签。
///
/// `category` 是 `'static`：这些分类是代码里的固定词表，不接受外部输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoTag {
    pub name: String,
    pub category: &'static str,
}

impl AutoTag {
    fn new(name: impl Into<String>, category: &'static str) -> Self {
        Self {
            name: name.into(),
            category,
        }
    }
}

/// 一条提示词关键词规则：`(关键词, 标签名, 分类)`。
///
/// 关键词一律小写；英文按词边界匹配，中文按子串匹配。顺序就是优先级：
/// 同一段提示词命中很多条时，靠前的先留下（总上限见 [`MAX_PROMPT_TAGS`]）。
const PROMPT_RULES: &[(&str, &str, &str)] = &[
    // ---------------- 风格 ----------------
    ("赛博朋克", "赛博朋克", "风格"),
    ("cyberpunk", "赛博朋克", "风格"),
    ("水墨", "水墨", "风格"),
    ("ink painting", "水墨", "风格"),
    ("ink wash", "水墨", "风格"),
    ("水彩", "水彩", "风格"),
    ("watercolor", "水彩", "风格"),
    ("油画", "油画", "风格"),
    ("oil painting", "油画", "风格"),
    ("动漫", "动漫", "风格"),
    ("二次元", "动漫", "风格"),
    ("anime", "动漫", "风格"),
    ("写实", "写实", "风格"),
    ("photorealistic", "写实", "风格"),
    ("realistic", "写实", "风格"),
    ("摄影", "摄影", "风格"),
    ("photography", "摄影", "风格"),
    ("像素", "像素", "风格"),
    ("pixel art", "像素", "风格"),
    ("插画", "插画", "风格"),
    ("illustration", "插画", "风格"),
    ("概念艺术", "概念艺术", "风格"),
    ("concept art", "概念艺术", "风格"),
    ("漫画", "漫画", "风格"),
    ("comic", "漫画", "风格"),
    ("卡通", "卡通", "风格"),
    ("cartoon", "卡通", "风格"),
    ("国风", "国风", "风格"),
    ("古风", "国风", "风格"),
    ("chinese style", "国风", "风格"),
    ("极简", "极简", "风格"),
    ("minimalist", "极简", "风格"),
    ("复古", "复古", "风格"),
    ("retro", "复古", "风格"),
    ("蒸汽波", "蒸汽波", "风格"),
    ("vaporwave", "蒸汽波", "风格"),
    ("奇幻", "奇幻", "风格"),
    ("fantasy", "奇幻", "风格"),
    ("科幻", "科幻", "风格"),
    ("sci-fi", "科幻", "风格"),
    ("哥特", "哥特", "风格"),
    ("gothic", "哥特", "风格"),
    ("超现实", "超现实", "风格"),
    ("surreal", "超现实", "风格"),
    ("电影感", "电影感", "风格"),
    ("cinematic", "电影感", "风格"),
    ("3d render", "3D", "风格"),
    ("3d", "3D", "风格"),
    // ---------------- 角色 ----------------
    ("1girl", "少女", "角色"),
    ("girl", "少女", "角色"),
    ("少女", "少女", "角色"),
    ("女孩", "少女", "角色"),
    ("1boy", "少年", "角色"),
    ("boy", "少年", "角色"),
    ("少年", "少年", "角色"),
    ("男孩", "少年", "角色"),
    ("woman", "女性", "角色"),
    ("女人", "女性", "角色"),
    ("女性", "女性", "角色"),
    ("man", "男性", "角色"),
    ("男人", "男性", "角色"),
    ("男性", "男性", "角色"),
    ("portrait", "肖像", "角色"),
    ("肖像", "肖像", "角色"),
    ("头像", "肖像", "角色"),
    ("character", "角色", "角色"),
    ("角色", "角色", "角色"),
    ("猫娘", "猫娘", "角色"),
    ("cat girl", "猫娘", "角色"),
    ("catgirl", "猫娘", "角色"),
    ("猫", "猫", "角色"),
    ("cat", "猫", "角色"),
    ("狗", "狗", "角色"),
    ("dog", "狗", "角色"),
    ("龙", "龙", "角色"),
    ("dragon", "龙", "角色"),
    ("精灵", "精灵", "角色"),
    ("elf", "精灵", "角色"),
    ("骑士", "骑士", "角色"),
    ("knight", "骑士", "角色"),
    ("女巫", "女巫", "角色"),
    ("witch", "女巫", "角色"),
    ("机器人", "机器人", "角色"),
    ("robot", "机器人", "角色"),
    ("机甲", "机甲", "角色"),
    ("mecha", "机甲", "角色"),
    ("天使", "天使", "角色"),
    ("angel", "天使", "角色"),
    ("恶魔", "恶魔", "角色"),
    ("demon", "恶魔", "角色"),
    ("士兵", "士兵", "角色"),
    ("soldier", "士兵", "角色"),
    ("法师", "法师", "角色"),
    ("mage", "法师", "角色"),
    ("长发", "长发", "角色"),
    ("long hair", "长发", "角色"),
    ("短发", "短发", "角色"),
    ("short hair", "短发", "角色"),
    ("蓝眼", "蓝眼", "角色"),
    ("blue eyes", "蓝眼", "角色"),
    ("红眼", "红眼", "角色"),
    ("red eyes", "红眼", "角色"),
    ("校服", "校服", "角色"),
    ("school uniform", "校服", "角色"),
    ("和服", "和服", "角色"),
    ("kimono", "和服", "角色"),
    ("泳装", "泳装", "角色"),
    ("swimsuit", "泳装", "角色"),
    ("盔甲", "盔甲", "角色"),
    ("armor", "盔甲", "角色"),
    ("剑", "剑", "角色"),
    ("sword", "剑", "角色"),
    ("翅膀", "翅膀", "角色"),
    ("wings", "翅膀", "角色"),
    // ---------------- 场景 ----------------
    ("风景", "风景", "场景"),
    ("landscape", "风景", "场景"),
    ("城市", "城市", "场景"),
    ("city", "城市", "场景"),
    ("街道", "街道", "场景"),
    ("street", "街道", "场景"),
    ("森林", "森林", "场景"),
    ("forest", "森林", "场景"),
    ("海洋", "海洋", "场景"),
    ("ocean", "海洋", "场景"),
    ("sea", "海洋", "场景"),
    ("山脉", "山脉", "场景"),
    ("mountain", "山脉", "场景"),
    ("夜晚", "夜晚", "场景"),
    ("night", "夜晚", "场景"),
    ("夜景", "夜晚", "场景"),
    ("日落", "日落", "场景"),
    ("sunset", "日落", "场景"),
    ("黄昏", "日落", "场景"),
    ("日出", "日出", "场景"),
    ("sunrise", "日出", "场景"),
    ("雨天", "雨", "场景"),
    ("rain", "雨", "场景"),
    ("雪景", "雪", "场景"),
    ("snow", "雪", "场景"),
    ("太空", "太空", "场景"),
    ("宇宙", "太空", "场景"),
    ("space", "太空", "场景"),
    ("室内", "室内", "场景"),
    ("indoor", "室内", "场景"),
    ("户外", "户外", "场景"),
    ("outdoor", "户外", "场景"),
    ("废墟", "废墟", "场景"),
    ("ruins", "废墟", "场景"),
    ("沙漠", "沙漠", "场景"),
    ("desert", "沙漠", "场景"),
    ("城堡", "城堡", "场景"),
    ("castle", "城堡", "场景"),
    ("寺庙", "寺庙", "场景"),
    ("temple", "寺庙", "场景"),
    ("教室", "教室", "场景"),
    ("classroom", "教室", "场景"),
    ("咖啡厅", "咖啡厅", "场景"),
    ("cafe", "咖啡厅", "场景"),
    ("海滩", "海滩", "场景"),
    ("沙滩", "海滩", "场景"),
    ("beach", "海滩", "场景"),
    ("天空", "天空", "场景"),
    ("sky", "天空", "场景"),
    ("云", "云", "场景"),
    ("cloud", "云", "场景"),
    ("花", "花", "场景"),
    ("flower", "花", "场景"),
    ("树", "树", "场景"),
    ("tree", "树", "场景"),
    ("水", "水", "场景"),
    ("water", "水", "场景"),
    ("火", "火", "场景"),
    ("fire", "火", "场景"),
    // ---------------- 画质 ----------------
    ("杰作", "杰作", "画质"),
    ("masterpiece", "杰作", "画质"),
    ("高质量", "高质量", "画质"),
    ("高品质", "高质量", "画质"),
    ("best quality", "高质量", "画质"),
    ("high quality", "高质量", "画质"),
    ("8k", "8K", "画质"),
    ("4k", "4K", "画质"),
    ("高细节", "高细节", "画质"),
    ("细节丰富", "高细节", "画质"),
    ("ultra detailed", "高细节", "画质"),
    ("highly detailed", "高细节", "画质"),
    ("detailed", "高细节", "画质"),
    ("清晰", "清晰", "画质"),
    ("锐利", "清晰", "画质"),
    ("sharp focus", "清晰", "画质"),
    ("hdr", "HDR", "画质"),
    // ---------------- 构图 ----------------
    ("特写", "特写", "构图"),
    ("close-up", "特写", "构图"),
    ("closeup", "特写", "构图"),
    ("全身", "全身", "构图"),
    ("full body", "全身", "构图"),
    ("半身", "半身", "构图"),
    ("upper body", "半身", "构图"),
    ("远景", "远景", "构图"),
    ("全景", "远景", "构图"),
    ("wide shot", "远景", "构图"),
    ("俯视", "俯视", "构图"),
    ("from above", "俯视", "构图"),
    ("bird's eye", "俯视", "构图"),
    ("仰视", "仰视", "构图"),
    ("from below", "仰视", "构图"),
    ("low angle", "仰视", "构图"),
    ("对称", "对称", "构图"),
    ("symmetry", "对称", "构图"),
    ("景深", "景深", "构图"),
    ("depth of field", "景深", "构图"),
    ("侧面", "侧面", "构图"),
    ("from side", "侧面", "构图"),
    ("profile", "侧面", "构图"),
    ("背影", "背影", "构图"),
    ("back", "背影", "构图"),
    ("坐姿", "坐姿", "构图"),
    ("sitting", "坐姿", "构图"),
    ("站姿", "站姿", "构图"),
    ("standing", "站姿", "构图"),
    ("躺姿", "躺姿", "构图"),
    ("lying", "躺姿", "构图"),
    ("看镜头", "看镜头", "构图"),
    ("looking at viewer", "看镜头", "构图"),
    // ---------------- 氛围 ----------------
    ("暗黑", "暗黑", "氛围"),
    ("dark", "暗黑", "氛围"),
    ("恐怖", "恐怖", "氛围"),
    ("horror", "恐怖", "氛围"),
    ("梦幻", "梦幻", "氛围"),
    ("dreamy", "梦幻", "氛围"),
    ("忧郁", "忧郁", "氛围"),
    ("melancholy", "忧郁", "氛围"),
    ("快乐", "快乐", "氛围"),
    ("开心", "快乐", "氛围"),
    ("happy", "快乐", "氛围"),
    ("史诗", "史诗", "氛围"),
    ("epic", "史诗", "氛围"),
    ("神秘", "神秘", "氛围"),
    ("mysterious", "神秘", "氛围"),
    ("温馨", "温馨", "氛围"),
    ("cozy", "温馨", "氛围"),
    ("戏剧性", "戏剧性", "氛围"),
    ("dramatic", "戏剧性", "氛围"),
    ("微笑", "微笑", "氛围"),
    ("smile", "微笑", "氛围"),
    ("哭泣", "哭泣", "氛围"),
    ("crying", "哭泣", "氛围"),
    ("生气", "生气", "氛围"),
    ("angry", "生气", "氛围"),
];

/// 提示词最多留几条标签。
///
/// 12 是"足够概括一张图、又不至于把标签库冲垮"的经验值：一条长提示词能命中几十条规则，
/// 全留下会让标签页里全是只出现一次的词。
const MAX_PROMPT_TAGS: usize = 12;

/// 一条运行最多从 LoRA 文件名里留几条（工作流里挂五六个 LoRA 很常见，全留太吵）。
const MAX_LORA_TAGS: usize = 4;

/// 模型家族词表：`(文件名里的小写片段, 展示名)`。
///
/// 只放**能确定**的家族；认不出来时走 [`cleaned_model_name`]（清理过的文件名），
/// 而不是硬塞一个 `SDXL`（把 Krea 标成 SDXL 比不标更糟）。
const MODEL_FAMILIES: &[(&str, &str)] = &[
    ("anima", "Anima"),
    ("flux", "FLUX"),
    ("sdxl", "SDXL"),
    ("sd_xl", "SDXL"),
    ("sd15", "SD 1.5"),
    ("sd_1.5", "SD 1.5"),
    ("sd-v1-5", "SD 1.5"),
    ("stable-diffusion-v1", "SD 1.5"),
    ("illustrious", "Illustrious"),
    ("noobai", "NoobAI"),
    ("pony", "Pony"),
    ("hunyuan", "混元"),
    ("qwen", "Qwen"),
    ("chroma", "Chroma"),
    ("hidream", "HiDream"),
    ("kolors", "Kolors"),
    ("playground", "Playground"),
    ("ltxv", "LTX"),
    ("ltx-video", "LTX"),
    ("cogvideo", "CogVideo"),
    ("mochi", "Mochi"),
    ("krea", "Krea"),
    ("wan2", "Wan"),
    ("wan_", "Wan"),
    ("wan-", "Wan"),
    ("aesthetic", "Aesthetic"),
];

/// 从解析结果推断标签（入库那条路用它）。
pub fn infer(parsed: &Parsed, nsfw: bool) -> Vec<AutoTag> {
    infer_parts(
        &parsed.positive,
        parsed.checkpoint.as_deref(),
        &parsed.loras,
        &parsed.kind,
        nsfw,
    )
}

/// 从字段推断标签（「给存量补标签」与单测用它）。
///
/// 所有字段都只读；`positive` 可以是空串（只根据模型 / 类型出标签）。
pub fn infer_parts(
    positive: &str,
    checkpoint: Option<&str>,
    loras: &[LoraRef],
    kind: &str,
    nsfw: bool,
) -> Vec<AutoTag> {
    let mut out: Vec<AutoTag> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut push = |name: &str, category: &'static str| -> bool {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 96 {
            return false;
        }
        if seen.insert(name.to_lowercase()) {
            out.push(AutoTag::new(name, category));
            return true;
        }
        false
    };

    // 类型：图片是绝大多数，不额外打标；视频 / 音频 / 混合才值得筛。
    match kind.trim().to_uppercase().as_str() {
        "VIDEO" => {
            push("视频", "类型");
        }
        "AUDIO" => {
            push("音频", "类型");
        }
        "MIXED" => {
            push("混合", "类型");
        }
        _ => {}
    }

    if let Some(checkpoint) = checkpoint {
        if let Some(name) = model_display(checkpoint) {
            push(&name, "模型");
        }
    }

    for lora in loras.iter().take(MAX_LORA_TAGS) {
        if let Some(name) = cleaned_model_name(&lora.name) {
            push(&name, "LoRA");
        }
    }

    let text = positive.to_lowercase();
    if !text.trim().is_empty() {
        let mut count = 0usize;
        for (keyword, tag, category) in PROMPT_RULES {
            if count >= MAX_PROMPT_TAGS {
                break;
            }
            if keyword_matches(&text, keyword) {
                if push(tag, category) {
                    count += 1;
                }
            }
        }
    }

    if nsfw {
        push("NSFW", "内容");
    }

    out
}

/// 底模的展示名：先认家族，认不出来就用清理过的文件名。
pub fn model_display(raw: &str) -> Option<String> {
    let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let stem = strip_model_ext(file);
    let lower = stem.to_lowercase();
    for (needle, family) in MODEL_FAMILIES {
        if keyword_matches(&lower, needle) {
            return Some((*family).to_string());
        }
    }
    cleaned_model_name(stem)
}

/// 清理模型 / LoRA 文件名：去扩展名、路径、下划线 / 连字符，压缩空白。
///
/// 返回 `None` = 清理完太短（`a.safetensors`）或像一串哈希，不值得做成标签。
fn cleaned_model_name(raw: &str) -> Option<String> {
    let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let stem = strip_model_ext(file);
    let cleaned = stem
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.chars().count() < 2 || cleaned.chars().count() > 64 {
        return None;
    }
    // 纯十六进制且足够长 = 多半是哈希 / 编号，不是模型名
    let hex = cleaned.chars().all(|c| c.is_ascii_hexdigit());
    if hex && cleaned.len() >= 16 {
        return None;
    }
    Some(cleaned)
}

fn strip_model_ext(name: &str) -> &str {
    let lower = name.to_lowercase();
    for ext in [
        ".safetensors",
        ".ckpt",
        ".pth",
        ".pt",
        ".gguf",
        ".bin",
        ".onnx",
    ] {
        if lower.ends_with(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// 关键词是否命中：中文按子串，英文按词边界。
///
/// 英文边界只把**字母**算作"同一个词"：`1girl` 能命中 `girl`（前面是数字），
/// 而 `location` 不会命中 `cat`（前面是字母）、`girls` 不会命中 `girl`（后面是字母）。
fn keyword_matches(text: &str, keyword: &str) -> bool {
    if keyword.is_empty() {
        return false;
    }
    if !keyword.is_ascii() {
        return text.contains(keyword);
    }
    let mut from = 0usize;
    while let Some(offset) = text[from..].find(keyword) {
        let start = from + offset;
        let end = start + keyword.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .map(|c| !c.is_ascii_alphabetic())
            .unwrap_or(true);
        let after_ok = text[end..]
            .chars()
            .next()
            .map(|c| !c.is_ascii_alphabetic())
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        // 从下一个字符继续找（不能停在同一个位置，否则死循环）
        let next = start
            + text[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        if next >= text.len() {
            break;
        }
        from = next;
    }
    false
}

/// 一次「给存量补标签」的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTagReport {
    /// 有新增标签的提示词条数（不是扫过的条数）。
    pub prompts: usize,
    /// 有新增标签的产物条数。
    pub media: usize,
    /// 新增的标签关联条数（提示词 + 产物）。
    pub tags: usize,
}

/// 给库里已有的提示词与产物补一遍自动标签（界面上的「为已有内容补标签」按钮走这里）。
///
/// 只追加、不删除；同一个标签已经在某条内容上就跳过。整个扫描在一个写事务里，
/// 中途出错不会留下"一半有标签、一半没有"的库。
pub fn apply_existing(state: &AppState) -> Result<AutoTagReport, ApiError> {
    state.write(|tx| {
        let mut report = AutoTagReport::default();

        let prompts: Vec<(i64, String, Option<String>, Option<String>, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, positive_prompt, checkpoint, loras, kind FROM prompts ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get(2)?,
                    row.get(3)?,
                    row.get::<_, Option<String>>(4)?
                        .unwrap_or_else(|| "IMAGE".into()),
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (prompt_id, positive, checkpoint, loras_text, kind) in prompts {
            let loras: Vec<LoraRef> = crate::web::models::parse_loras(loras_text.as_deref())
                .into_iter()
                .map(|l| LoraRef {
                    name: l.name,
                    weight: l.weight,
                })
                .collect();
            let inferred = infer_parts(&positive, checkpoint.as_deref(), &loras, &kind, false);
            let added = repo::append_prompt_auto_tags(tx, prompt_id, &inferred)?;
            if added > 0 {
                report.prompts += 1;
                report.tags += added;
            }

            let media_rows: Vec<(i64, String, bool)> = {
                let mut stmt = tx.prepare(
                    "SELECT id, kind, nsfw FROM media_assets WHERE prompt_id = ?1 ORDER BY id",
                )?;
                let rows = stmt.query_map([prompt_id], |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, Option<String>>(1)?
                            .unwrap_or_else(|| "IMAGE".into()),
                        row.get::<_, i64>(2).unwrap_or(0) != 0,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (media_id, media_kind, nsfw) in media_rows {
                let media_tags =
                    infer_parts(&positive, checkpoint.as_deref(), &loras, &media_kind, nsfw);
                let added = repo::append_media_auto_tags(tx, media_id, &media_tags)?;
                if added > 0 {
                    report.media += 1;
                    report.tags += added;
                }
            }
        }

        repo::refresh_use_counts(tx)?;
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tags: &[AutoTag]) -> Vec<&str> {
        tags.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn prompt_keywords_become_categorized_tags() {
        let tags = infer_parts(
            "1girl, cyberpunk city at night, ink painting",
            None,
            &[],
            "IMAGE",
            false,
        );
        let names = names(&tags);
        assert!(names.contains(&"少女"), "{names:?}");
        assert!(names.contains(&"赛博朋克"), "{names:?}");
        assert!(names.contains(&"城市"), "{names:?}");
        assert!(names.contains(&"夜晚"), "{names:?}");
        assert!(names.contains(&"水墨"), "{names:?}");
        assert!(
            tags.iter()
                .any(|t| t.name == "赛博朋克" && t.category == "风格"),
            "{tags:?}"
        );
    }

    #[test]
    fn english_keywords_respect_word_boundaries() {
        assert!(keyword_matches("1girl, solo", "girl"), "数字前缀要能命中");
        assert!(
            !keyword_matches("location", "cat"),
            "location 里的 cat 不算"
        );
        assert!(
            !keyword_matches("girlfriend", "girl"),
            "girlfriend 里的 girl 不算"
        );
        assert!(keyword_matches("cat_ears", "cat"), "下划线是词边界");
        assert!(keyword_matches("best quality, 8k", "best quality"));
        assert!(keyword_matches("cyberpunk city", "cyberpunk"));
        assert!(!keyword_matches("cyberpunkish", "cyberpunk"));
    }

    #[test]
    fn model_family_is_recognized_from_checkpoint_filename() {
        assert_eq!(
            model_display("anima-base-v1.0.safetensors").as_deref(),
            Some("Anima")
        );
        assert_eq!(
            model_display("flux1-dev-fp8.safetensors").as_deref(),
            Some("FLUX")
        );
        assert_eq!(
            model_display("D:/models/sd_xl_base_1.0.safetensors").as_deref(),
            Some("SDXL")
        );
        assert_eq!(
            model_display("juggernautXL_v9.safetensors").as_deref(),
            Some("juggernautXL v9"),
            "认不出家族时用清理过的文件名，不能硬猜"
        );
        assert_eq!(model_display("a.safetensors"), None, "太短的文件名不成标签");
    }

    #[test]
    fn loras_type_and_nsfw_add_tags() {
        let loras = vec![LoraRef {
            name: "add_detail_v2.safetensors".into(),
            weight: 0.8,
        }];
        let tags = infer_parts("", Some("krea2.safetensors"), &loras, "VIDEO", true);
        let names = names(&tags);
        assert!(names.contains(&"Krea"), "{names:?}");
        assert!(names.contains(&"add detail v2"), "{names:?}");
        assert!(names.contains(&"视频"), "{names:?}");
        assert!(names.contains(&"NSFW"), "{names:?}");
        assert!(
            tags.iter()
                .any(|t| t.name == "视频" && t.category == "类型"),
            "{tags:?}"
        );
    }

    #[test]
    fn duplicate_tags_are_collapsed_case_insensitively() {
        // 底模叫 Anima、LoRA 也叫 anima：只留先到的模型标签
        let loras = vec![LoraRef {
            name: "anima.safetensors".into(),
            weight: 1.0,
        }];
        let tags = infer_parts("", Some("anima.safetensors"), &loras, "IMAGE", false);
        assert_eq!(names(&tags), vec!["Anima"]);
    }

    #[test]
    fn prompt_tags_are_capped() {
        let prompt = PROMPT_RULES
            .iter()
            .map(|(keyword, _, _)| *keyword)
            .collect::<Vec<_>>()
            .join(", ");
        let tags = infer_parts(&prompt, None, &[], "IMAGE", false);
        assert!(
            tags.len() <= MAX_PROMPT_TAGS,
            "标签上限 {MAX_PROMPT_TAGS}，实际 {}",
            tags.len()
        );
    }
}
