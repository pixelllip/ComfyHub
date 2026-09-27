# AI 文案台：人工校验 + 批量修改（设计）

> **缘起（2026-09-27 用户提的"大问题"）**："我怎么人工校验 AI 文案和批量修改他们呢？"
> 澄清之后的范围是两类：
> ① **规则类文案** —— 系统提示词 / 工具描述 / skill 正文；
> ② **前端设置页里的各种 AI 注解** —— 那些解释"这个开关干什么、密钥怎么存、预算怎么算"的话。
>
> 现状是：这些文案**散在四处、各自有各自的真源**，谁都没法一眼看全，更没有"我读过并确认"
> 这个事实 —— 于是"文案说错了"只能靠用户撞上（例如工具描述里写着"提交时也会跑一次审核"，
> 而代码当时根本没跑，见总纲 §24）。

---

## 1. 一句话设计

**不搬真源，加一层"覆盖 + 校验"**：

```
默认文本（真源：Rust 常量 / 磁盘文件 / Dart 字面量）
        │  运行时查一次覆盖层
        ▼
copy_overrides(key, text, base_hash, updated_at, verified_at, note)   ← 用户改过的那几段
        │
        ▼
实际下发的文本（run-kit / 界面）
```

三条不动的纪律：

1. **真源不变**：`nsfw_prompt.rs` 的正文、`desktop_tools.rs` 的描述、skill 文件仍是"出厂默认"。
   覆盖层只是"这台机器上用户改成了什么"。
2. **改了要能被追溯**：`base_hash` 记下"我改的时候默认值长什么样"；
   上游一改默认值（`base_hash` 变了），那条**自动变回"待重新校验"**。
3. **规则类文案一改就必须 bump 版本**（R12 / R13）：`prompt.*` / `tool.*` → `PROMPT_VERSION` + `kitVersion`；
   `ui.*` → 只 bump `uiCopyVersion`（不进 run-kit，手机不需要知道）。

---

## 2. key 命名空间（唯一真源）

| key | 真源 | 怎么改 | 备注 |
| --- | --- | --- | --- |
| `prompt.s1` … `prompt.s20` | `server-rs/src/nsfw_prompt.rs` 的 `PERSONA_RAW`（按 `§N` 切段） | 覆盖层 | 段号稳定；段内占位符（`«MAX_WEB»` 等）**不许改坏**（机器校验） |
| `prompt.nsfw` | 同文件里 `NSFW_RAW`（那段越狱人格） | 覆盖层 | ⚠ **改它之前先问用户**（AGENTS §10 明文） |
| `prompt.title` / `prompt.continue` | run-kit 下发的两条附加指令 | 覆盖层 | 与 `titleInstruction` / `continueInstruction` 同源 |
| `tool.<name>.description` | `server-rs/src/desktop_tools.rs` / `tools.rs`（含 MCP 39 个） | 覆盖层 | 模型看到的就是它；改了 = 真的改了行为 |
| `tool.<name>.schema` | 同上 | **只读**（不改） | 改 schema 等于改接口，不属于"文案" |
| `skill.<name>.body` | `<storage>\ai\skills\<name>\SKILL.md` 或 `skills\builtin\<name>` | **写文件**（不是覆盖层） | 已有投放口与 `register_skill`；批量改要带备份 |
| `ui.<page>.<key>` | Dart 侧字面量（`copy('ui.settings.nsfw.note', '默认…')`） | 覆盖层 | 离线时用默认值；在线时用桌面覆盖 |

**为什么 `ui.*` 也归桌面管**：用户明确要"前端设置里面的各种 AI 注解"能统一校验/批量改。
默认值仍留在 Dart（离线可用），但**承诺性文案**（"密钥只写不读"、"越界写被拒"这类）
必须能被桌面统一改口径 —— 否则两端各说各话，而那是 R13 最怕的漂移。

---

## 3. 表与端点

```sql
CREATE TABLE copy_overrides (
  key         TEXT PRIMARY KEY,      -- §2 的命名空间
  text        TEXT NOT NULL,         -- 覆盖后的文本（当前生效）
  base_hash   TEXT NOT NULL,         -- 写这条时"默认文本"的 sha256（上游一改就变）
  note        TEXT,                  -- 用户自己的备注（"为什么改"）
  updated_at  TEXT NOT NULL,
  verified_at TEXT                   -- 人工校验过的时间；文本一改就清空
);
```

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/copy/catalog?scope=&q=&verified=` | 清单：每条 `{key, scope, title, default, current, overridden, baseHash, baseChanged, verifiedAt, refs}` |
| `GET` | `/api/copy/{key}` | 详情（默认文本 + 当前文本，供界面直接 diff） |
| `PUT` | `/api/copy/{key}` | 改一条（body：`{text, note?}`）；`skill.*` 走文件写回 |
| `DELETE` | `/api/copy/{key}` | 还原默认（删覆盖层那一行） |
| `POST` | `/api/copy/{key}/verify` | 标记"我读过并确认"（写 `verified_at`） |
| `POST` | `/api/copy/batch` | 批量：**`dryRun` 先出 diff，再 apply**（见 §4） |
| `GET` | `/api/copy/audit?limit=` | 追加式审计：谁、什么时候、把哪条从什么改成了什么 |

`refs` 是"这条文案在哪被用/在哪定义"（文件:行 / 端点 / 界面位置）—— **没有它就没法校验**：
用户要能点一下跳过去看上下文。

---

## 4. 批量修改的语义（这是"批量"的全部价值）

```jsonc
POST /api/copy/batch
{
  "keys": ["tool.comfy_submit.description", "prompt.s17"],
  "op": "replace" | "regex_replace" | "append" | "prepend" | "delete_lines" | "insert_after",
  "find": "…",            // replace / delete_lines / insert_after 用
  "replace": "…",
  "regex": "…",           // regex_replace 用（Go/Rust 正则；**默认不是多行**，要 `(?m)` 自己写）
  "dryRun": true          // ★ 永远先 dryRun
}
→ { "items": [ { "key", "before", "after", "diff", "changed": bool, "blocked": "原因?" } ] }
```

四条硬规矩：

1. **dryRun 默认 true**：没有"预览 diff"这一步的批量改文案就是赌博。
2. **一条改坏了不连坐**：逐条独立 apply，回 `blocked` 原因（占位符改坏 / 触发 R13 检查 / 文件不可写）。
3. **批量不许越过"人工校验"**：apply 之后那些 key 的 `verified_at` 一律清空（人读过的不是这一版）。
4. **`prompt.nsfw` 参与批量时要二次确认**（界面弹一次；它是用户有意注入的那一段）。

---

## 5. 机器校验（自动跑，放在 `blocked` 里）

| 检查 | 为什么 |
| --- | --- |
| 占位符完整性 | `«MAX_WEB»` / `«MAX_COMFY»` / `«MAX_TOOL_STEPS»` 这类是**替换点**，改坏的表现是正文里漏出原始标记或数字不对 |
| R13 白名单 | 文案里点名的工具必须真在 `IMPLEMENTED` 里（§17.4 / §19.1 那两次"描述是真的吗"就是这么查出来的） |
| 段落位移 | 系统提示按 `§N` 切段：**段号是 key**，删掉一整段要让后面顺延（否则 key 与内容错位） |
| 长度预算 | 系统提示有大小上限（进每次请求）；超了要如实拒绝而不是静默截断 |
| Markdown 结构 | 与生成器同款：`trim_indent` 后可比对、不许出现未闭合的代码块 |

---

## 6. 人工校验的形态（界面）

「AI 文案台」（设置页入口，桌面为主）：

- **左列表**：按 scope 分组（系统提示 / 工具描述 / skill / 界面注解），每行带三态徽标
  `未校验` / `已校验` / **`默认值变过，请重看`**（`baseChanged`）。
- **右详情**：默认 vs 当前**并排 diff**（默认值只读）；「还原默认」「标记已校验（可附一句备注）」。
- **批量面板**：勾多条 → 选操作 → **先看 diff** → 应用；应用前若含 `prompt.nsfw` 二次确认。
- **筛选**：`scope`、关键字、`只看待校验`、`只看我改过的`。
- **导出 / 导入 JSON**：整份覆盖层可导出，用编辑器批改后再导入（走同一个 dryRun 校验）。

手机端：**只读**看清单（"这些规则由桌面统一下发"），不提供批量改 —— 手机上做文案审校是受罪。

---

## 7. 落地分步

| 步 | 内容 | 验收 |
| --- | --- | --- |
| C1 | 表 + 覆盖层读取（`prompt.s*` 分段 + `tool.*` + run-kit 下发覆盖后的文本）+ catalog/detail/put/delete/verify | `cargo test`：覆盖生效、`base_hash` 变了要报 `baseChanged`、改文案要 bump 版本 |
| C2 | 批量端点（六种 op + dryRun diff + 逐条 blocked + 审计） | 用例：正则替换只动命中处、占位符改坏被拦、`prompt.nsfw` 要 `confirm` |
| C3 | `skill.*` 写回（带备份 + 回滚） | 用例：改完文件内容变、非法 frontmatter 被拒、备份可还原 |
| C4 | `ui.*`：Dart 侧 `copy(key, 默认值)` 查覆盖层 + `GET /api/copy/ui` | `flutter test`：没有覆盖时用默认值、有覆盖时用覆盖值、离线用默认值 |
| C5 | 界面（文案台页 + 批量面板 + 导出导入） | widget 用例：三态徽标、diff 预览、批量改完 `verified_at` 被清空 |
| C6 | 把现有文案接进来（系统提示各条 / 全部工具描述 / 设置页注解） | 清单条数对得上（`tool.*` 条数 == `IMPLEMENTED` + MCP 39），无遗漏 |

---

## 8. 明确不做

- **不做"自动重写文案"**（让模型自己改系统提示词）：那是把 R13 的唯一真源交给模型。
  批量操作全部由人指定 find/replace。
- **不做"改了就立刻生效于在飞的 Run"**：Run 开始时快照一次（与策略同一条纪律），
  下一轮 Run 才用新文案。
- **不把覆盖层塞进同步范围**（第一版）：它是桌面侧的编辑产物；
  手机只读清单一律走 `/api/copy/catalog`。要不要下行覆盖层，等有人真在手机上改再说。
