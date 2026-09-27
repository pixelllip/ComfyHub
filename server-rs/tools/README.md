# `server-rs/tools/` —— P4 移植工具（**不是运行期依赖**）

这里的脚本只在**把 Kotlin 侧的东西搬到 Rust** 时用。它们不参与构建、不参与运行，
`cargo test` 也不会碰它们。

| 脚本 | 干什么 | 产物 |
| --- | --- | --- |
| `gen_prompts.py` | ⛔ **已停用：只拒绝、不写**（见下节）。原来从 Kotlin `NsfwPrompt.kt` / `HarnessRunner.kt` 机械提取提示词正文（照抄 `trimIndent()` 语义，`${...}` 插值换成哨兵）；提取助手留着当**历史参考** | **无**（退出码 2，一个字节都不写） |
| `gen_mobile_tools.py` | 从 Kotlin `/api/ai/run-kit` 的**真实响应**里取手机工具（name / description / inputSchema） | `src/mobile_tools.rs` |
| `check_signature.py` | 按 Kotlin `RunKit.Signature.canonical()` 重算 `kitVersion` 的内容签名，与库里 `app_settings['ai.runkit.hash']` 比对 | 只打印结论 |
| `compare_run_kit.py` | 同一份库 + 同一份 storage 上，canon 比较 Rust 与 Kotlin 的 `/api/ai/run-kit`（只归一化「当前时间」那一行） | 只打印结论 |
| `gen_capability_rules.py` | 从 Kotlin `ModelCapabilityCatalog.kt` 的 `rules` 表**机械提取** 118 条内置能力规则（模式 / 模态 / 工具 / 推理 / 思考档位 / 思考方言，**顺序即语义**） | `src/capability_rules.rs`（`--check` 只校验不写） |

## ⛔ 系统提示正文**不再生成**（`gen_prompts.py` 已停用，2026-09-27）

`src/nsfw_prompt.rs` **就是**系统提示正文的真源，别再跑生成 —— 跑一次是**一次回归**：

1. 脚本读的 `server/src/main/kotlin/…` **已经不存在**（Kotlin 树在 P4 C-6 退役时搬到
   `legacy/server/`）；就算把路径改到 `legacy/`，下面两条照样成立。
2. `PROMPT_VERSION` 会**从 v33 退回 v31**（它从 `HarnessRunner.kt` 读版本号），
   并整段删掉"Rust 为什么从 v32 走到 v33 / 为什么跳过 v31"那段注释。
3. 第 19 条（图像编辑）那批 **2026-09-27 的实测结论只在 Rust 侧**（Kotlin 里
   `mattingModels` / `先想能不能不抠` / `别默默降级` / `krea2 不适合` 一个都没有）——
   生成一次，"贴人物图首选 AI 软遮罩 / 缺模型指引用户下载 / krea2 不适合做编辑"就全没了。

（第 20 条配方规则也曾是 Kotlin 独有；**P4 第 69 步**已按原文补进 Rust 的 v33，
所以它不再构成差异。）所以 `main()` 现在只打印这份理由并**非零退出**；
要改正文就改 `server-rs/src/nsfw_prompt.rs`，并 bump `PROMPT_VERSION`（红线 R12）。

## 为什么要"生成"而不是手抄（**只对还没停用的脚本成立**）

这几段正文加起来 5 万多字符（`PERSONA_BLOCK` 10584 + `NSFW_BLOCK` 31695），
外加 7 个工具的 JSON Schema。**手抄一个字的差错就是"两端规则不一致"**（红线 R13），
而这类差错在界面上**看不出来** —— 模型只是照着一份略微不同的规则说话；
生成脚本的价值就是让正文逐字一致（判据是 `compare_run_kit.py` 的"canon 后零差异"）。

这条理由对 `gen_mobile_tools.py` / `gen_capability_rules.py` **仍然成立**（它们另有真源）；
对**系统提示正文**已经不成立 —— 2026-09-27 起 Rust 侧的那个文件就是真源（见上节）。

## 怎么跑（**历史**：P4 收尾后 Kotlin 已退役，这段只在"回退旧后端"时才有意义）

```powershell
# ① 起一个 Kotlin 后端对着**库副本**（不要对着真库：它会把 kitVersion 写进 app_settings）
$env:JAVA_HOME = 'D:\Program Files\Android Studio\jbr'      # 21~23，见 AGENTS §4
$env:COMFYHUB_PORT = '8090'
$env:COMFYHUB_STORAGE = '<某个 storage 副本>'
$env:COMFYHUB_JDBC_URL = 'jdbc:sqlite:<某个 comfyhub.db 副本>'
$env:COMFYHUB_PROJECT_ROOT = '<项目根>'
server\build\install\comfy-hub-server\bin\comfy-hub-server.bat

# ② 抓一份基线（gen_mobile_tools.py 读的就是它）
Invoke-WebRequest 'http://127.0.0.1:8090/api/ai/run-kit' -OutFile .run\runkit\baseline\kotlin-run-kit.json

# ③ 生成 + 编译 + 验收
python server-rs\tools\gen_mobile_tools.py
cargo test --manifest-path server-rs\Cargo.toml
# 再起 Rust 版（同一份库/storage，另一个端口）抓一次，然后比：
python server-rs\tools\compare_run_kit.py
```

⚠ 这条流程里**没有** `gen_prompts.py` 了 —— 它现在会拒绝并退出 2（理由见上节）。
⚠ `compare_run_kit.py` 从 2026-09-27 起**必然报出差异**（Rust v33 / Kotlin v31，
且 §19 的两批内容不同）：那是**已知的、有意的分叉**，看的时候要按"哪些差异是设计好的"判，
别把系统提示正文那几段差异当成回归。

## ⚠ 两个"看起来没事、其实很严重"的坑（都真踩过）

1. **Rust raw string 的 `#` 在开引号之前**：是 `r#"…"#`，不是 `r"#…"#`；
   而且**开引号与正文之间不能加换行** —— 那是字符串的一部分，会让每段正文都以换行开头
   （结果是"下发的 JSON Schema 非法"或"系统提示两端差一个字符"）。
2. **`kitVersion` 的签名必须逐字节一致**：canonical 里含 `tools[]` 的序列化文本
   （键序 + 紧凑分隔符）。Rust 侧如果换成 `serde_json::to_string(&Value)`
   （`serde_json::Map` 默认**按键排序**），两端的 hash 就不同，于是
   **各自都认为"规则变了"→ 互相把版本号顶上去**（一边 bump，另一边又 bump）。
   所以 `src/run_kit.rs` 里那份 `tools_json` 是手工拼的，`check_signature.py` 钉住公式。
