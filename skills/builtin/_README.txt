内置 Skills 目录（只读）
========================

把随项目分发的 Skill 放在这里，一个 Skill 一个目录：

    skills/builtin/<name>/SKILL.md
    skills/builtin/<name>/references/...     # 可选，正文里可以引用相对路径

也支持平铺写法：`skills/builtin/<name>.md`。

规则（与 `server/src/main/kotlin/com/comfyhub/ai/tools/SkillStore.kt` 一致）：

- `<name>` 必须是小写 kebab-case（`^[a-z0-9][a-z0-9-]{0,63}$`），且与 frontmatter 里的
  `name` 一致；
- `SKILL.md` 开头是 `---` 围起来的 frontmatter：`name`（必需）、`description`（必需）、
  可选 `whenToUse` / `version` / `user-invocable` / `disable-model-invocation`；
- 同名的用户 Skill（`<storage>/ai/skills/<name>`）**优先级更高**：用户版本胜出，
  这一份会被隐藏并在界面上标"冲突"；
- 这个目录里的 Skill **不能通过界面或 AI 删除**（`DELETE /api/ai/skills/{name}` 返回
  `SKILL_READONLY`），只能停用；
- 本目录里目前有两个**随包分发**的内置 Skill：
  `img2img-reference`（把用户发来的图投放进 ComfyUI 的 input 目录，再覆盖 LoadImage）；
  `krea-2`（Krea 2 提示词规则 + RAW/Turbo 官方参数 + 本机 ComfyUI 落地，
  官方原文逐字放在它的 `references/` 里，出处/许可见那份 `SOURCES.md`）。
  发布包能带上它们靠 `packaging/manifest.json` 的 `skills` 组件（source/target 都是 `skills`）。
- 用户自己的 Skill **不放这里**：拷进投放口 `<storage>\ai\skills` 就算装好
  （后端启动时 / 点「重新扫描」会自动给没有 frontmatter 的文件补登记）。
  注意 `%USERPROFILE%\.dsh` 已经没有任何代码读了（连"从 DSH 导入"按钮都删了）。

> 注意：本文件故意用 `.txt` 后缀 —— 扫描器只认 `*.md`，一个 `README.md` 会被当成
> 一个"缺少 description 的非法 Skill"列到界面上。
