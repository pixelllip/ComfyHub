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
- **本目录不会被自动创建/填充**：项目当前没有随包分发的内置 Skill，用户可以从
  `%USERPROFILE%\.dsh\skills` 一键导入（右侧栏「从 DSH 导入」）。
  要给发布包带上内置 Skill，记得同时在 `packaging/manifest.json` 里登记这个目录。

> 注意：本文件故意用 `.txt` 后缀 —— 扫描器只认 `*.md`，一个 `README.md` 会被当成
> 一个"缺少 description 的非法 Skill"列到界面上。
