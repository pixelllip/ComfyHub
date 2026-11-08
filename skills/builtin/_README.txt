内置 Skills 目录（只读）
========================

把随项目分发的 Skill 放在这里，一个 Skill 一个目录：

    skills/builtin/<name>/SKILL.md
    skills/builtin/<name>/references/...     # 可选，正文里可以引用相对路径

也支持平铺写法：`skills/builtin/<name>.md`。

规则（与后端扫描器的规则一致）：

- `<name>` 必须是小写 kebab-case（`^[a-z0-9][a-z0-9-]{0,63}$`），且与 frontmatter 里的
  `name` 一致；
- `SKILL.md` 开头是 `---` 围起来的 frontmatter：`name`（必需）、`description`（必需）、
  可选 `whenToUse` / `version` / `user-invocable` / `disable-model-invocation`；
- 同名的用户 Skill（`<storage>/ai/skills/<name>`）**优先级更高**：用户版本胜出，
  这一份会被隐藏并在界面上标"冲突"；
- 这个目录里的 Skill **不能通过界面或 AI 删除**（`DELETE /api/ai/skills/{name}` 返回
  `SKILL_READONLY`），只能停用；
- 本目录里目前有这些**随包分发**的内置 Skill：
  `comfy-workflow-guide`（工作流第一站：判断 t2i / i2i / edit / inpaint / video 用途、
   给用户选现有工作流、复制副本再改、TextGenerate 摘线）；
  `img2img-reference`（把用户发来的图投放进 ComfyUI 的 input 目录，再覆盖 LoadImage）；
  `inpaint-composite`（图像编辑的局部重绘：保背景换人物 / 换背景保人物 / 两阶段"先生成人物再换上去"。
   配套工具 `comfy_make_mask` + `comfy_use_media`；工作流用**用户自己的**（本软件 2026-10-28 起不再随包带模板），
   贴人那条链要求 `easy imageRemBg` + `refine_foreground=true`）；
  `comfy-workflow-audit`（工作流审核的判定与修法）；
  `lora-training`（LoRA 数据集 / 打标 / 训练配方与验收）；
  `workflow-recipe`（换新模型：学配方 / 绑模型 / 四档结论）；
  `krea-2`（Krea 2 提示词规则 + RAW/Turbo 官方参数 + 从**用户自己保存的 krea2 工作流**落地，
   官方原文逐字放在它的 `references/` 里，出处/许可见那份 `SOURCES.md`）；
  `moss-image`（MOSS-Image 提示词 / Layout JSON 单套 schema / 参数 + 官方 ComfyUI 模板四个
   变体与 TextGenerate 摘线）；
  `card-writer`（角色卡 / 画风卡写作：卡里只存**一版自然语言正面提示词**；
   角色 600+ token 全身上下、画风 100~200 token **起**（下限不是上限，复杂画风按
   上色 / 打光 / 面部·头发·衣物·其他物体的材质分部位写长）；要 Danbooru tag 版时当场另翻、不存卡）；
  `r2i-prompt`（参考图 + 一到多段 `[CHARACTER CARD]` + `[STYLE SPEC]` → 一整段 300~400 词提示词；
    工作台输入区 `+` 菜单 / `@` 插卡（角色卡可多选 = 多角色同框），卡片正文逐字进槽位、
    **卡本身不进成品**：画风卡是锁定渲染规格，面部 / 身体 / 头发 / 衣物的具体渲染写进对应部位的句子，
    结尾只留 ≤25 词总括；参考图的表情按可见面部配置如实写出；成品正文放进三反引号 text 围栏）。
  这些 Skill 随发布包一起分发。

  **手机端只有其中四份**：`r2i-prompt` / `card-writer` / `krea-2` / `moss-image`
  会在 App 第一次启动时由 `SkillSeeder` 从 APK 资源种进本机投放口
  （`<filesDir>/ai/skills`，只种一次、绝不覆盖用户改过的文件）。其余几份要 ComfyUI
  在场上，手机本地既不能跑也不该假装能跑 —— 所以**不种**。改名 / 换清单要同步改
  `android/harness/src/main/kotlin/com/comfyhub/harness/tools/SkillSeeder.kt` 的
  `SEEDED_SKILLS`。
- 用户自己的 Skill **不放这里**：拷进投放口 `<storage>\ai\skills` 就算装好
  （后端启动时 / 点「重新扫描」会自动给没有 frontmatter 的文件补登记）。

> 注意：本文件故意用 `.txt` 后缀 —— 扫描器只认 `*.md`，一个 `README.md` 会被当成
> 一个"缺少 description 的非法 Skill"列到界面上。
