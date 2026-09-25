//! 工具权限策略 —— **run-kit 需要的那一半**（P4 第 22 步，`/api/ai/run-kit`）。
//!
//! # 为什么只有一半
//!
//! 完整的 `ToolPolicy`（`resolveRead` / `resolveWrite`、`FORBIDDEN_SEGMENTS` 禁写段、
//! 真实路径判定、越界拒绝）属于**文件工具**（`read_file` / `write_file`，还没搬）。
//! 这里做的是 run-kit 与**工具循环**真正读到的这几件事：
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
//! ⚠ **别在这里补一个"半份的路径判定"**：`resolveWrite` 的语义是"越界一律拒绝"，
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

/// `app_settings` 里策略 JSON 的键（Kotlin `ToolPolicyConfig.KEY`）。
pub const KEY: &str = "ai.tools.policy";

pub const PERMISSION_ASK: &str = "ask";
pub const PERMISSION_FULL: &str = "full";

/// 出厂写白名单（只有一个目录：用户要求"默认不能修改 comfy 目录以外的内容"）。
pub const DEFAULT_WRITE_DIR: &str = "comfyui";
/// 出厂读白名单（产物目录只读）。
pub const DEFAULT_READ_DIRS: &[&str] = &["comfyui", "storage"];

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
        Self { project_root, config, write_roots, read_roots, permission_mode }
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
        Ok(Self::new(state.project_root(), config))
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
fn normalize_lexical(path: &Path) -> PathBuf {
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
}
