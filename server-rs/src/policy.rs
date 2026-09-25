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

use crate::routes::{ApiError, AppState};
use crate::tools::ToolFailure;

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
    /// 单次回复最多**工具轮数**（Kotlin `ToolPolicyConfig.maxToolSteps`，出厂 16）。
    ///
    /// ⚠ 到顶**不是失败**：那一轮不执行工具、如实告诉用户"已达轮数上限、再发一条就能接着做"
    /// （Kotlin `HarnessRunner` 的原话就是这句）。写成 `TOOL_LOOP_LIMIT` 把整轮 Run 弄失败，
    /// 用户已经看到的那段回答就会挂上一个红叉 —— 而那件事本来只是"这轮到此为止"。
    pub max_tool_steps: i64,
    /// 单个 Run 最多**工具调用次数**（出厂 48）——与轮数是两条线，见
    /// [`crate::tools::budget_exceeded_note`] 里为什么措辞必须分开。
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
pub const DEFAULT_MAX_TOOL_STEPS: i64 = 16;
/// 单个 Run 的工具调用次数上限（`ToolPolicyConfig.DEFAULT_MAX_CALLS_PER_RUN`）。
pub const DEFAULT_MAX_CALLS_PER_RUN: i64 = 48;

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
    /// 这一份是**每次现算的探测结果**（[`crate::comfy_dirs::auto_read_roots`]），不落库 ——
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
        let raw: Option<String> = state.with(|conn| {
            Ok(conn
                .query_row("SELECT v FROM app_settings WHERE k = ?1", [KEY], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten())
        })?;
        let config = raw
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .and_then(|text| serde_json::from_str::<ToolPolicyConfig>(text).ok())
            .unwrap_or_default();
        let project_root = state.project_root();
        // 探测失败 / 配置里没有输出目录 → 空表，**不是错误**（读白名单照常只有用户配的那些）
        let configured_output = crate::ai_settings::configured_output_dir(state).ok().flatten();
        let auto_read =
            crate::comfy_dirs::auto_read_roots(&project_root, configured_output.as_deref());
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
            return std::fs::canonicalize(path).ok().map(crate::comfy_dirs::plain_path);
        }
        let mut tail: Vec<String> = Vec::new();
        let mut cursor = Some(path);
        let mut base: Option<PathBuf> = None;
        while let Some(current) = cursor {
            if current.exists() {
                base = std::fs::canonicalize(current).ok().map(crate::comfy_dirs::plain_path);
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
mod tests {
    use super::*;

    fn policy(json: &str) -> ToolPolicy {
        let config: ToolPolicyConfig = serde_json::from_str(json).unwrap();
        ToolPolicy::new(PathBuf::from(r"D:\proj"), config)
    }

    #[test]
    fn 出厂白名单与_kotlin_一致() {
        let p = policy("{}");
        assert_eq!(p.write_roots, vec![PathBuf::from(r"D:\proj\comfyui")]);
        assert_eq!(
            p.read_roots,
            vec![PathBuf::from(r"D:\proj\comfyui"), PathBuf::from(r"D:\proj\storage")]
        );
        assert_eq!(p.permission_mode, PERMISSION_ASK);
        assert!(!p.full_permission());
    }

    #[test]
    fn 白名单展开认相对与绝对两种写法() {
        let p = policy(r#"{"writeRoots":["comfyui","D:\\other\\out","a/../b"],"readRoots":["  storage  "],"permissionMode":"full"}"#);
        assert_eq!(
            p.write_roots,
            vec![
                PathBuf::from(r"D:\proj\comfyui"),
                PathBuf::from(r"D:\other\out"),
                PathBuf::from(r"D:\proj\b"),
            ]
        );
        assert_eq!(p.read_roots, vec![PathBuf::from(r"D:\proj\storage")]);
        assert!(p.full_permission());
    }

    #[test]
    fn 权限档认不出来就回落询问() {
        // 配置写坏（比如拼成 "FULL "、或者老库里塞了别的词）不能放开权限
        assert_eq!(policy(r#"{"permissionMode":"FULL "}"#).permission_mode, PERMISSION_ASK);
        assert_eq!(policy(r#"{"permissionMode":"whatever"}"#).permission_mode, PERMISSION_ASK);
        assert!(policy(r#"{"permissionMode":"full"}"#).full_permission());
    }

    #[test]
    fn 生效权限按权限档放宽但不动_deny() {
        let ask = policy("{}");
        assert_eq!(ask.access_for("web_search", "ASK"), ToolAccess::Ask);
        assert_eq!(ask.access_for("list_skills", "ALLOW"), ToolAccess::Allow);

        let full = policy(r#"{"permissionMode":"full"}"#);
        assert_eq!(full.access_for("web_search", "ASK"), ToolAccess::Allow, "全权限档下 ask 变 allow");
        assert_eq!(full.access_for("x", "DENY"), ToolAccess::Deny, "deny 是禁用，不放宽");
        assert_eq!(full.access_for("x", "???"), ToolAccess::Allow, "认不出的出厂档当 ask 处理");
    }

    #[test]
    fn 逐工具覆盖优先于出厂档() {
        let p = policy(r#"{"overrides":{"web_search":"deny","list_skills":"ask"}}"#);
        assert_eq!(p.access_for("web_search", "ASK"), ToolAccess::Deny);
        assert_eq!(p.access_for("list_skills", "ALLOW"), ToolAccess::Ask);
        // 覆盖值写坏了 → 回落出厂档（不是"随便给一个"）
        let broken = policy(r#"{"overrides":{"web_search":"nope"}}"#);
        assert_eq!(broken.access_for("web_search", "ASK"), ToolAccess::Ask);
    }

    /// 两条预算与 Kotlin 的出厂值一致，而且**老配置（没这两个键）走默认**。
    #[test]
    fn 工具预算的出厂值与_kotlin_一致() {
        let p = policy("{}");
        assert_eq!(p.config.max_tool_steps, 16);
        assert_eq!(p.config.max_calls_per_run, 48);
        // 老库里冻着的配置没有这两个键 → 必须落到出厂值，不是 0
        let legacy = policy(r#"{"writeRoots":["comfyui"],"permissionMode":"full"}"#);
        assert_eq!(legacy.config.max_tool_steps, DEFAULT_MAX_TOOL_STEPS);
        assert_eq!(legacy.config.max_calls_per_run, DEFAULT_MAX_CALLS_PER_RUN);
        // 用户自己调过的值要生效
        let custom = policy(r#"{"maxToolSteps":8,"maxCallsPerRun":16}"#);
        assert_eq!(custom.config.max_tool_steps, 8);
        assert_eq!(custom.config.max_calls_per_run, 16);
    }

    // ------------------------------------------------------------------
    //  路径准入（P4 第 40 步）
    // ------------------------------------------------------------------

    /// 每个用例一个**真实存在**的临时根（路径准入要解真实路径，光有字符串不够）。
    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("comfyhub-policy-{tag}-{unique}-{seq}"));
        std::fs::create_dir_all(root.join("comfyui")).unwrap();
        std::fs::create_dir_all(root.join("storage")).unwrap();
        root
    }

    fn policy_at(root: &Path, json: &str) -> ToolPolicy {
        ToolPolicy::new(root.to_path_buf(), serde_json::from_str(json).unwrap())
    }

    #[test]
    fn 写只能在写白名单里读只能在读白名单里() {
        let root = temp_root("roots");
        let p = policy_at(&root, "{}");
        // 写：出厂只有 comfyui
        let inside = p.resolve_write("comfyui/a/b.txt").unwrap();
        assert!(inside.starts_with(root.join("comfyui")), "{}", inside.display());
        // 不存在也照样解析（write_file 的目标通常还不存在）
        assert!(inside.ends_with("b.txt"));
        // storage 是**读**白名单，不是写白名单
        let denied = p.resolve_write("storage/x.txt").unwrap_err();
        assert_eq!(denied.code, "PATH_DENIED");
        assert!(denied.message.contains("只允许在"), "{}", denied.message);
        // 读：comfyui 与 storage 都行
        assert!(p.resolve_read("storage/x.txt").is_ok());
        assert!(p.resolve_read("comfyui/x.txt").is_ok());
        // 项目根之外
        assert_eq!(p.resolve_read("../outside.txt").unwrap_err().code, "PATH_DENIED");
        // 空路径是参数错，不是越界
        assert_eq!(p.resolve_write("   ").unwrap_err().code, "INVALID_ARGUMENT");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn 禁段与用户白名单无关() {
        let root = temp_root("forbidden");
        // 用户把白名单放宽到**项目根**（最宽的一种写法）
        let p = policy_at(&root, r#"{"writeRoots":["."],"readRoots":["."]}"#);
        assert!(p.resolve_write("comfyui/ok.txt").is_ok());
        for bad in [".git/config", ".run/x.log", "node_modules/a/b.js", ".mysql/data/x"] {
            let err = p.resolve_write(bad).unwrap_err();
            assert_eq!(err.code, "PATH_DENIED", "{bad} 竟然放行了");
            assert!(err.message.contains("运行期数据 / 依赖目录"), "{}", err.message);
            // 读也一样拒（措辞要跟 verb 对上，别让"读被拒"看起来像"能读不能改"）
            let read_err = p.resolve_read(bad).unwrap_err();
            assert!(read_err.message.contains("拒绝读"), "{}", read_err.message);
        }
        // 大小写不敏感（Windows 上 `.GIT` 一样要拒）
        assert_eq!(p.resolve_write(".GIT/config").unwrap_err().code, "PATH_DENIED");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn 目录根本身不能当写入目标() {
        let root = temp_root("dirroot");
        let p = policy_at(&root, "{}");
        let err = p.resolve_write("comfyui").unwrap_err();
        assert_eq!(err.code, "PATH_DENIED");
        assert!(err.message.contains("目录根本身"), "{}", err.message);
        // 带尾斜杠写法归一到同一个目录，照样拒
        assert!(p.resolve_write("comfyui/").is_err());
        // **读**目录根本身是允许的（list_dir 的默认路径就是它）
        assert!(p.resolve_read("comfyui").is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn 自动发现的目录只放宽读() {
        let root = temp_root("auto");
        let outside = root.join("ComfyUI-Installs").join("ComfyUI");
        std::fs::create_dir_all(outside.join("user").join("default").join("workflows")).unwrap();
        let p = policy_at(&root, "{}").with_auto_read_roots(vec![outside.clone()]);
        // 读：自动发现的目录可以（用户 bug ④）
        assert!(p
            .resolve_read(&outside.join("user").join("default").join("workflows").join("a.json").display().to_string())
            .is_ok());
        // 写：**一点都没放宽**（那只放宽读）
        let err = p.resolve_write(&outside.join("x.json").display().to_string()).unwrap_err();
        assert_eq!(err.code, "PATH_DENIED");
        // 去重 + 保序（Kotlin `.distinct()`）
        let dup = policy_at(&root, "{}").with_auto_read_roots(vec![root.join("storage"), outside.clone()]);
        assert_eq!(dup.effective_read_roots(), vec![root.join("comfyui"), root.join("storage"), outside]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn 真实路径解掉符号链接与逐字前缀() {
        let root = temp_root("real");
        let p = policy_at(&root, "{}");
        std::fs::create_dir_all(root.join("comfyui").join("sub")).unwrap();
        std::fs::write(root.join("comfyui").join("sub").join("a.txt"), b"x").unwrap();
        let resolved = p.resolve_write("comfyui/sub/a.txt").unwrap();
        // Windows 的 canonicalize 会给 `\\?\D:\…`，闸门内部必须已经去掉那个前缀
        assert!(!resolved.to_string_lossy().starts_with(r"\\?\"), "{}", resolved.display());
        // 词法上的 `..` 要被消掉（`comfyui/../comfyui/a.txt` 仍然合法）
        assert!(p.resolve_write("comfyui/../comfyui/b.txt").is_ok());
        // 但消完落在白名单外就要拒
        assert!(p.resolve_write("comfyui/../storage/x.txt").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 非法字符这一档：Java 的 `Paths.get` 会抛 `InvalidPathException`，
    /// Rust 的 `PathBuf` **什么都收** —— 不自己挡就会一路走到 `create_dir_all` 才报一个
    /// 看不懂的 IO 错误（而 Kotlin 那边回的是 `PATH_DENIED`）。
    #[test]
    fn 非法字符按无法解析拒绝() {
        let root = temp_root("badchars");
        let p = policy_at(&root, "{}");
        let err = p.resolve_write("comfyui/a\u{0}b.txt").unwrap_err();
        assert_eq!(err.code, "PATH_DENIED");
        assert!(p.resolve_write("comfyui/a?b.txt").is_err());
        assert!(p.resolve_write("comfyui/a|b.txt").is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
