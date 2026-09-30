//! 工具权限策略（P4 第 22 步建 run-kit 需要的那一半，**第 40 步补齐路径准入**）。
//!
//! # 两条腿
//!
//! 1. **权限档 / 白名单展开 / 逐工具覆盖 / 两条预算**（第 22 步）：run-kit 与工具循环读的；
//! 2. **路径准入**（第 40 步）：[`ToolPolicy::resolve_read`] / [`ToolPolicy::resolve_write`]
//!    —— 文件工具（`read_file` / `write_file` / `list_dir`）的**唯一**闸门。
//!
//! ⚠ 第 2 条是**安全边界**，不是"要不要问一下"：Kotlin `ToolPolicy.resolve` 的语义是
//! "越界一律拒绝"，这里逐行对照搬过来（禁写段、真实路径、目录根本身不可写）。
//!
//! 1. **白名单展开**（`writeRoots` / `readRoots` → 绝对路径）—— 系统提示里要如实写出
//!    "只能写哪、只能读哪"，那是模型判断"能不能动这个文件"的唯一依据（Kotlin `render:1483`）；
//! 2. **权限档**（`ask` / `full`）—— `full` 时提示词要多一段"这次不必等批准"，
//!    并且 `ask` 的工具在渲染时**不再标「需要用户批准」**；
//! 3. **逐工具覆盖**（`overrides`）—— 标「已被禁用」（`deny`）以及"可调用的工具"清单里
//!    被策略剔除的那些；
//! 4. **两条预算**（`maxToolSteps` / `maxCallsPerRun`）—— 工具循环的轮数与调用数上限
//!    （P4 第 31 步起真的生效，见 `executor` 的 `step` / `calls`）。
//!
//! ⚠ **别在这里补一个"半份的路径判定"**：`resolve_write` 的语义是"越界一律拒绝"，
//! 抄一半（比如只看前缀、不做真实路径）会让写操作通过一个比 Kotlin 松的闸门 ——
//! 那比"还没搬"危险得多。
//!
//! # 一处刻意的重复（照抄 Kotlin 的口径）
//!
//! 阈值（`MAX_COMFY_QUERIES_PER_RUN` / `MAX_WEB_QUERIES_PER_RUN`）在这里是**常量**
//! —— 它就是"提示词里那个数字"的唯一真源（Kotlin 侧用 `${...}` 插值，Rust 侧用
//! `nsfw_prompt::resolve` 替换哨兵）。**别在提示词正文里再写一遍数字。**

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::web::routes::{ApiError, AppState};
use crate::tools::tools::ToolFailure;

/// `app_settings` 里策略 JSON 的键（Kotlin `ToolPolicyConfig.KEY`）。
pub const KEY: &str = "ai.tools.policy";

pub const PERMISSION_ASK: &str = "ask";
pub const PERMISSION_FULL: &str = "full";

/// 出厂写白名单（只有一个目录：用户要求"默认不能修改 comfy 目录以外的内容"）。
pub const DEFAULT_WRITE_DIR: &str = "comfyui";
/// 出厂读白名单（产物目录只读）。
pub const DEFAULT_READ_DIRS: &[&str] = &["comfyui", "storage"];

/// **任何工具都不许读也不许写**的路径段（Kotlin `ToolPolicyConfig.FORBIDDEN_SEGMENTS`）。
///
/// 这些是运行期数据与依赖：`.git`（版本库）、`.mysql`（老库，现在只剩搬迁来源）、
/// `.run`（日志 / 探测脚本）、`node_modules`。**与用户白名单无关** ——
/// 用户就算把白名单放宽到项目根，这几段照样拒绝。
pub const FORBIDDEN_SEGMENTS: [&str; 4] = [".git", ".mysql", ".run", "node_modules"];

/// 单次读取上限：**2MB**（Kotlin `ToolPolicyConfig.DEFAULT_MAX_READ_BYTES`）。
///
/// 以前是 256KB —— 一份界面格式的工作流 JSON 轻轻松松就超过它，`read_file`
/// 只能读到前面一截，模型自然改不动。
pub const DEFAULT_MAX_READ_BYTES: i64 = 2 * 1024 * 1024;
/// 单次写入上限：**4MB**（同上 `DEFAULT_MAX_WRITE_BYTES`）。
///
/// ⚠ 这两项与 `maxComfyQueriesPerRun` / `maxWebQueriesPerRun` 一样，在 Kotlin 侧是
/// **界面上不可改、由 `normalizeStored` 强制回落到代码常量**的：老库里冻着
/// `maxReadBytes:262144`，只改代码默认值对老用户完全无效。所以这里**根本不读配置**，
/// 直接用常量（读了反而会与 Kotlin 不一致）。
pub const DEFAULT_MAX_WRITE_BYTES: i64 = 4 * 1024 * 1024;

/// 一次回复里最多主动查 ComfyUI 几次（**提示词里那个数字的真源**）。
pub const MAX_COMFY_QUERIES_PER_RUN: i64 = 9;
/// 一次回复里最多联网搜索几次（同上）。
pub const MAX_WEB_QUERIES_PER_RUN: i64 = 8;

/// 工具权限档（`allow` / `ask` / `deny`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    Allow,
    Ask,
    Deny,
}

impl ToolAccess {
    /// 解析配置里的档位；**认不出来一律当作"没配"**（`None`）——
    /// 权限这种事不能因为配置写坏了就放开（Kotlin `ToolAccess.parse`）。
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw?.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// `app_settings` 里那一行 JSON（`ai.tools.policy`）。
///
/// 只声明**现在真的读**的字段：Kotlin 那边还有 `maxComfyQueriesPerRun` / `maxReadBytes` /
/// `maxWriteBytes` …（要 ComfyUI / 文件工具的那些批次才读，见文件头）。宽松解析照抄
/// `AppJson`（`ignoreUnknownKeys`），所以这里**不能**用 `deny_unknown_fields`。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolPolicyConfig {
    pub write_roots: Vec<String>,
    pub read_roots: Vec<String>,
    pub overrides: BTreeMap<String, String>,
    pub permission_mode: String,
    /// 单次回复最多**工具轮数**（`ToolPolicyConfig.maxToolSteps`，出厂 **25**）。
    ///
    /// ⚠ 到顶**不是失败**：那一轮不执行工具、如实告诉用户"已达轮数上限、再发一条就能接着做"
    /// （Kotlin `HarnessRunner` 的原话就是这句）。写成 `TOOL_LOOP_LIMIT` 把整轮 Run 弄失败，
    /// 用户已经看到的那段回答就会挂上一个红叉 —— 而那件事本来只是"这轮到此为止"。
    pub max_tool_steps: i64,
    /// 单个 Run 最多**工具调用次数**（出厂 48）——与轮数是两条线，见
    /// [`crate::tools::tools::budget_exceeded_note`] 里为什么措辞必须分开。
    pub max_calls_per_run: i64,
}

impl Default for ToolPolicyConfig {
    fn default() -> Self {
        Self {
            write_roots: Vec::new(),
            read_roots: Vec::new(),
            overrides: BTreeMap::new(),
            // 空串 = 没配 → `permission_mode()` 回落到 ask
            permission_mode: String::new(),
            max_tool_steps: DEFAULT_MAX_TOOL_STEPS,
            max_calls_per_run: DEFAULT_MAX_CALLS_PER_RUN,
        }
    }
}

/// 单次回复的工具轮数上限（`ToolPolicyConfig.DEFAULT_MAX_TOOL_STEPS`）。
///
/// **16 → 25（2026-09-27 用户拍板）**：真机那次"没有可用工作流时让 AI 自己搭一份"
/// （总纲 §17）用了 **22 次调用**，其中一半是侦察（读权重索引 / 查节点契约 / 搜模型），
/// 16 轮会在**最后一步**（`comfy_submit`）把它掐断。上界放到 [`MAX_TOOL_STEPS_LIMIT`]，
/// 让"搭图"这类长链路够用，又不会离 `max_calls_per_run`（48）太远。
pub const DEFAULT_MAX_TOOL_STEPS: i64 = 25;
/// `maxToolSteps` 的**旧出厂值**（2026-09-27 那次"16 → 25"之前的默认）。
///
/// 为什么要单独认它（2026-09-28 从用户库里查出来的真问题）：那次改动只动了**默认值**，
/// 而用户的 `app_settings.ai.tools.policy` 里**早就把 16 落成了一份显式配置**
/// （实测：`{"…","maxToolSteps":16,…}`）—— 显式配置永远盖过默认值，默认怎么改都不生效。
/// 真机现场就是会话 `8a306e34` 的最后一轮被掐断，原话
/// 「已达到本次回复的工具轮数上限 16」—— 明明代码里已经写着 25。
///
/// 所以这里把"**正好等于旧出厂值**"的那一份当成"从未显式设置过"，升到新默认；
/// 用户自己挑过的其它值（8 / 40 …）一个字节都不动。
pub const LEGACY_DEFAULT_MAX_TOOL_STEPS: i64 = 16;
/// `maxToolSteps` 的硬上界（设置页能选到、`PUT /api/ai/tools/policy` 也只会夹到这里）。
///
/// 与 [`DEFAULT_MAX_CALLS_PER_RUN`] 同量级：轮数再多也不会超过"总调用次数"那条线。
pub const MAX_TOOL_STEPS_LIMIT: i64 = 48;
/// 单个 Run 的工具调用次数上限（`ToolPolicyConfig.DEFAULT_MAX_CALLS_PER_RUN`）。
pub const DEFAULT_MAX_CALLS_PER_RUN: i64 = 48;

/// 把"**旧出厂值** 16"升级成新默认（纯函数，用例直接喂配置，不用起库）。
///
/// 判据只有一个：`max_tool_steps` **正好等于** [`LEGACY_DEFAULT_MAX_TOOL_STEPS`]。
/// 别把这条写成"小于新默认就升"——那会把用户自己挑的 8 / 12 一起改掉，
/// 而他明明是有意调低的（设置页里那些值都在选项里）。
pub fn migrate_legacy_max_tool_steps(config: &mut ToolPolicyConfig) {
    if config.max_tool_steps == LEGACY_DEFAULT_MAX_TOOL_STEPS {
        config.max_tool_steps = DEFAULT_MAX_TOOL_STEPS;
    }
}

/// 那次升级的**一次性标记**（写过就不再升级，见 [`ToolPolicy::load`]）。
pub const MIGRATION_KEY: &str = "ai.tools.policy.migrated.maxtoolsteps16to25";

/// 把升级后的值写回 `app_settings`（**合并**原 JSON，保住我们这一版不认识的键 —— 与
/// `ai_settings::save_config` 同一条纪律），并落一次性标记。
///
/// 返回 `Err` 时调用方**不当失败**：升级是"顺手做的好事"，写不进去顶多是下次再算一遍，
/// 绝不能让一次设置迁移把工具策略的读取整个搞挂。
fn persist_migrated_max_tool_steps(
    state: &AppState,
    raw: &Option<String>,
    steps: i64,
) -> Result<(), ApiError> {
    let mut map = raw
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    map.insert("maxToolSteps".into(), serde_json::json!(steps));
    let text = serde_json::Value::Object(map).to_string();
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![KEY, text],
        )?;
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![MIGRATION_KEY, "1"],
        )?;
        Ok(())
    })
}

/// 生效的策略（白名单已展开成绝对路径）。
#[derive(Debug, Clone)]
pub struct ToolPolicy {
    pub project_root: PathBuf,
    pub config: ToolPolicyConfig,
    /// 用户配的写白名单（空 → 出厂默认 `<根>\comfyui`）。
    pub write_roots: Vec<PathBuf>,
    /// 用户配的读白名单（空 → `<根>\comfyui` + `<根>\storage`）。
    pub read_roots: Vec<PathBuf>,
    /// **自动发现的**只读目录（Kotlin `ToolPolicy.autoReadRoots`）。
    ///
    /// 与 `read_roots` 刻意分开两件事：`read_roots` 是**用户配过的**（落库、界面可改）；
    /// 这一份是**每次现算的探测结果**（[`crate::comfy::comfy_dirs::auto_read_roots`]），不落库 ——
    /// 用户换了 ComfyUI 的安装位置、或者头一次装上 ComfyUI，下一次工具调用就自动认得
    /// （用户 bug ④）。它**只影响读**，写仍然只认 [`Self::write_roots`]。
    pub auto_read_roots: Vec<PathBuf>,
    pub permission_mode: String,
}

impl ToolPolicy {
    /// 从配置构造（纯函数，用例直接喂 JSON，不用起库）。
    pub fn new(project_root: PathBuf, config: ToolPolicyConfig) -> Self {
        let write_entries: Vec<String> = if config.write_roots.is_empty() {
            vec![DEFAULT_WRITE_DIR.to_string()]
        } else {
            config.write_roots.clone()
        };
        let read_entries: Vec<String> = if config.read_roots.is_empty() {
            DEFAULT_READ_DIRS.iter().map(|s| s.to_string()).collect()
        } else {
            config.read_roots.clone()
        };
        let write_roots = expand(&write_entries, &project_root);
        let read_roots = expand(&read_entries, &project_root);
        let permission_mode = match config.permission_mode.trim() {
            PERMISSION_FULL => PERMISSION_FULL.to_string(),
            _ => PERMISSION_ASK.to_string(),
        };
        Self { project_root, config, write_roots, read_roots, auto_read_roots: Vec::new(), permission_mode }
    }

    /// 挂上"自动发现的只读目录"（Kotlin 构造参数 `autoReadRoots`）。
    pub fn with_auto_read_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.auto_read_roots = roots;
        self
    }

    /// 实际生效的读白名单 = 用户配的 + 自动发现的（**去重、保持顺序**，Kotlin `distinct()`）。
    pub fn effective_read_roots(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::with_capacity(self.read_roots.len() + self.auto_read_roots.len());
        for root in self.read_roots.iter().chain(self.auto_read_roots.iter()) {
            if !out.contains(root) {
                out.push(root.clone());
            }
        }
        out
    }

    /// 自动允许（无需批准）档：`ask` 的工具不必再等批准。
    pub fn full_permission(&self) -> bool {
        self.permission_mode == PERMISSION_FULL
    }

    /// 生效权限：用户覆盖 → 出厂档 → **再按权限档放宽**（只把 `ask` 变成 `allow`；
    /// `deny` 不动 —— 那是"禁用这个工具"，不是"要不要问一下"）。
    pub fn access_for(&self, tool_name: &str, default_access: &str) -> ToolAccess {
        let base = ToolAccess::parse(self.config.overrides.get(tool_name).map(String::as_str))
            .unwrap_or_else(|| ToolAccess::parse(Some(default_access)).unwrap_or(ToolAccess::Ask));
        if self.full_permission() && base == ToolAccess::Ask {
            ToolAccess::Allow
        } else {
            base
        }
    }

    /// 读配置（读失败 / JSON 坏 → 出厂默认，**绝不因为设置坏了就放开权限**）。
    ///
    /// 顺带把**自动发现的只读目录**挂上：`read_file` 能不能读用户自己的 ComfyUI 工作流
    /// 文件，全靠这一条（用户 bug ④ 要的正是"读放宽、写不放宽"）。
    pub fn load(state: &AppState) -> Result<Self, ApiError> {
        // 一次读两行：策略本身 + 这次升级的**一次性标记**
        let (raw, migrated) = state.with(|conn| {
            let policy = conn
                .query_row("SELECT v FROM app_settings WHERE k = ?1", [KEY], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten();
            let done = conn
                .query_row(
                    "SELECT v FROM app_settings WHERE k = ?1",
                    [MIGRATION_KEY],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .is_some();
            Ok((policy, done))
        })?;
        let config = raw
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .and_then(|text| serde_json::from_str::<ToolPolicyConfig>(text).ok())
            .unwrap_or_default();
        // 旧出厂值 16 → 新默认（见 [`LEGACY_DEFAULT_MAX_TOOL_STEPS`]）。
        //
        // **一次性**：标记写下去之后就不再动它 —— 否则用户哪天在设置页里特意选了 16，
        // 下一次读配置又会被悄悄改回 25（那是把一个明确的选择当成没选）。
        let mut config = config;
        if !migrated && config.max_tool_steps == LEGACY_DEFAULT_MAX_TOOL_STEPS {
            config.max_tool_steps = DEFAULT_MAX_TOOL_STEPS;
            // 顺手把升级后的值写回库：不写回去的话"读一次升一次"看起来没差别，
            // 但下次用户点保存写回来的仍是那份旧的 16。
            let _ = persist_migrated_max_tool_steps(state, &raw, config.max_tool_steps);
        }
        let project_root = state.project_root();
        // 探测失败 / 配置里没有输出目录 → 空表，**不是错误**（读白名单照常只有用户配的那些）
        let configured_output = crate::ai::ai_settings::configured_output_dir(state).ok().flatten();
        let auto_read =
            crate::comfy::comfy_dirs::auto_read_roots(&project_root, configured_output.as_deref());
        Ok(Self::new(project_root, config).with_auto_read_roots(auto_read))
    }

    // ------------------------------------------------------------------
    //  路径准入（Kotlin `resolveRead` / `resolveWrite`，P4 第 40 步）
    // ------------------------------------------------------------------

    /// 读路径：必须落在**生效读白名单**内（用户配的 + 自动发现的 ComfyUI 目录）。
    pub fn resolve_read(&self, raw: &str) -> Result<PathBuf, ToolFailure> {
        let roots = self.effective_read_roots();
        self.resolve(raw, &roots, "读")
    }

    /// 写路径：必须落在 `write_roots` 内，且不能落在禁写段。
    ///
    /// ⚠ 自动发现的目录**不在**这条路上（那只放宽读）。
    pub fn resolve_write(&self, raw: &str) -> Result<PathBuf, ToolFailure> {
        self.resolve(raw, &self.write_roots, "写")
    }

    /// 准入的完整过程（Kotlin `resolve` 逐行对照）：
    /// 空 / 非法字符 → 归一化 → 禁段 → **真实路径** → 白名单前缀 → "目录根不能当写目标"。
    ///
    /// 顺序不能改：先判禁段再取真实路径是可以的（禁段只看词法路径），但**白名单判定必须在
    /// 真实路径之后** —— 在真实路径之前判，符号链接就能把越界写伪装成合法写。
    fn resolve(&self, raw: &str, roots: &[PathBuf], verb: &str) -> Result<PathBuf, ToolFailure> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(ToolFailure::new("INVALID_ARGUMENT", "路径不能为空"));
        }
        if text.contains('\u{0}') {
            return Err(ToolFailure::new("PATH_DENIED", "路径包含非法字符"));
        }
        let candidate = self.candidate_path(text)?;

        self.reject_forbidden(&candidate, verb)?;

        let real = self
            .real_path(&candidate)
            .ok_or_else(|| ToolFailure::new("PATH_DENIED", format!("{verb} 路径不可解析：{text}")))?;

        let hit = roots.iter().find(|root| real.starts_with(root)).ok_or_else(|| {
            ToolFailure::new(
                "PATH_DENIED",
                format!(
                    "拒绝{verb} {text}：只允许在 {} 之内（可在「设置 → AI 工具权限」里调整）",
                    roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
                ),
            )
        })?;

        // Java 侧同一条：把**目录根本身**当写入目标会让下一次 list_dir / 覆盖语义变得毫无
        // 意义（实测踩过：一次 write_file 的 path 传成白名单目录，结果"写成功了"）
        if real == *hit && verb == "写" {
            return Err(ToolFailure::new(
                "PATH_DENIED",
                format!("拒绝把目录根本身当作写入目标：{text}"),
            ));
        }
        Ok(real)
    }

    /// 配置里的原始路径 → 绝对 + 词法归一化（Kotlin `Paths.get(...).normalize().toAbsolutePath()`）。
    fn candidate_path(&self, text: &str) -> Result<PathBuf, ToolFailure> {
        // Java 的 `Paths.get` 在 Windows 上会为 `<>:"|?*` 与控制字符抛 `InvalidPathException`。
        // Rust 的 `PathBuf` 什么都收，所以这一层要自己挡 —— 不挡的话这些路径会一路走到
        // `create_dir_all` 才报一个看不懂的 IO 错误（而 Kotlin 那边回的是 `PATH_DENIED`）。
        let invalid = text.chars().any(|c| {
            c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')
        });
        if invalid {
            return Err(ToolFailure::new("PATH_DENIED", format!("路径无法解析：{text}")));
        }
        let path = Path::new(text);
        let joined = if path.is_absolute() { path.to_path_buf() } else { self.project_root.join(path) };
        Ok(normalize_lexical(&joined))
    }

    /// 禁段判定（Kotlin `rejectForbidden`）：**大小写不敏感**（Windows 上 `.GIT` 一样要拒）。
    ///
    /// 路径不在项目根之内时 Kotlin 的 `relativize` 会给出 `..` 开头的相对路径 ——
    /// 那里面不可能命中禁段，所以这里 `strip_prefix` 失败就直接放行到白名单判定。
    fn reject_forbidden(&self, path: &Path, verb: &str) -> Result<(), ToolFailure> {
        let Ok(relative) = path.strip_prefix(&self.project_root) else { return Ok(()) };
        let hit = relative
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
                _ => None,
            })
            .find(|seg| FORBIDDEN_SEGMENTS.contains(&seg.as_str()));
        let Some(hit) = hit else { return Ok(()) };
        Err(ToolFailure::new(
            "PATH_DENIED",
            // 措辞要跟 verb 对上：这些段读写都不许碰，写成"不可改"会让一次**读取**被拒
            // 看起来像"我能读、只是不能改"（Kotlin 那边实测被这条消息误导过一次）
            format!(
                "拒绝{verb} {}：{hit} 是本项目的运行期数据 / 依赖目录，\
                 任何工具都不许读也不许写（与用户白名单无关）",
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            ),
        ))
    }

    /// 真实路径（Kotlin `realPath`）：存在就解掉符号链接；
    /// 不存在就**沿父目录往上找到第一个存在的祖先**，再拼回剩余部分。
    ///
    /// 为什么"不存在"也要解：`write_file` 的目标多半还不存在，只判词法路径的话
    /// 一个指向白名单之外的符号链接就能骗过闸门。
    fn real_path(&self, path: &Path) -> Option<PathBuf> {
        if path.exists() {
            // ⚠ Windows 的 `canonicalize` 返回逐字前缀 `\\?\D:\…`，而 Kotlin 的
            // `toRealPath()` 返回 `D:\…` —— 这个前缀会原样出现在工具结果里（还会被模型
            // 贴进下一次调用），是"同一个目录、两种写法"那类最难查的不一致（P4 第 14 步
            // 的实测）。统一过 `plain_path`。
            return std::fs::canonicalize(path).ok().map(crate::comfy::comfy_dirs::plain_path);
        }
        let mut tail: Vec<String> = Vec::new();
        let mut cursor = Some(path);
        let mut base: Option<PathBuf> = None;
        while let Some(current) = cursor {
            if current.exists() {
                base = std::fs::canonicalize(current).ok().map(crate::comfy::comfy_dirs::plain_path);
                break;
            }
            let name = current.file_name()?.to_string_lossy().to_string();
            tail.push(name);
            cursor = current.parent();
        }
        let mut out = base?;
        for name in tail.iter().rev() {
            out.push(name);
        }
        Some(normalize_lexical(&out))
    }
}

/// 配置里的目录 → 绝对路径（相对项目根）。照抄 Kotlin `ToolPolicy.expand`：
/// `Paths.get(raw)` → 是绝对路径就用它，否则 `projectRoot.resolve(...)` → `normalize().toAbsolutePath()`。
///
/// ⚠ **空串不跳过**：Kotlin 那边 `projectRoot.resolve("")` 就是项目根本身
/// （`Paths.get("")` 是空路径），跳过它会与 Kotlin 打印出不同的白名单。
fn expand(entries: &[String], project_root: &Path) -> Vec<PathBuf> {
    entries
        .iter()
        .map(|raw| {
            let text = raw.trim();
            let path = Path::new(text);
            let joined = if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) };
            normalize_lexical(&joined)
        })
        .collect()
}

/// 词法归一化（`Path.normalize()` 的对应物）：去掉 `.`、就地消掉 `..`，**不碰文件系统**。
///
/// `pub(crate)`：捕获入库解析 ComfyUI 给的 `subfolder` 时也要它（P4 第 43 步）——
/// 那条路径来自远端返回值，不能直接拼了就用。
pub(crate) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests;
