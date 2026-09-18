# 来源 / 版本 / 许可

本目录是内置 skill `krea-2` 的**官方原文**存放处（模型自己读不到这里：
本项目的读路径白名单默认只有 `comfyui` 与 `storage`，见 `ToolPolicyConfig.DEFAULT_READ_DIRS`）。
采集日期统一为 **2026-09**（采集当天 `main` 分支的内容）。

| 本地文件 | 上游 | 许可 / 备注 |
| --- | --- | --- |
| `official-skill-krea-2.md` | <https://github.com/krea-ai/skills> → `krea-generate/references/models/krea-2.md` | **MIT**（Copyright (c) 2026 Krea AI）。Krea 官方 Agent Skill 套件里专写 Krea 2 的那一份；逐字保留。 |
| `official-prompting.md` | <https://github.com/krea-ai/krea-2/blob/main/docs/prompting.md> | Krea 官方开源模型仓库（Krea 2 Community License）。逐字保留，**示例配图未随包分发**（原图在 `assets/samples/`）。 |
| `official-expansion-system-prompt.txt` | <https://github.com/krea-ai/krea-2/blob/main/docs/expansion.txt> | 同上。官方给出的"把短提示词扩写成 Krea 2 提示词"的 system prompt。 |
| `official-comfyui-krea-2.md` | <https://docs.comfy.org/tutorials/image/krea/krea-2> | ComfyUI 官方文档；**要点摘录**（非全文），含模型文件表、默认 8 步、风格参考管线与 9 个风格 LoRA。 |

其它被引用但未落盘的官方材料（写 `SKILL.md` 时参考过）：

- Krea 官方模型仓库 README（RAW 52 步 / cfg 3.5、Turbo 8 步 / cfg 0 / mu 1.15、1K~2K）：
  <https://github.com/krea-ai/krea-2> → `README.md`
- Krea 官方安全说明：<https://github.com/krea-ai/krea-2/blob/main/docs/safety.md>
- krea.ai 官方文档《Krea 2》（Medium / Large / Turbo、Srefs、情绪板、生成滑杆、Creativity）：
  <https://krea.mintlify.dev/docs/cn/user-guide/features/krea-2>
- Krea 官方 skill 套件（`krea-generate` / `krea-marketing` / `krea-motion`）：
  <https://github.com/krea-ai/skills>（MIT）

## 更新方式

官方改版后要同步：把上游对应文件重新抓下来覆盖本目录，并更新上表的采集日期；
如果官方 skill 的 frontmatter / 字段表变了，`SKILL.md` 正文（尤其是"云端独有能力"那张表）也要跟着改。
