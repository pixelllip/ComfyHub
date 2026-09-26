package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * **把一条配方绑到一个（新）模型上**（W3，2026-09-26）：产出"该写哪几个 `overrides` 键"，
 * 外加"哪些槽位本机没有 / 目录不对 / 实例看不到"。
 *
 * 这是用户那个问题的落点：出了新模型，**不用重搭工作流** —— 把已有的那条骨架
 * （比如 Krea 2 的参考图编辑骨架）绑到新底模上，跑的还是同一条图。
 *
 * ## 一条纪律（沿用 `WorkflowFit` 的取向）：**只报不改**
 *
 * 这个对象**不提交、不写文件、不改工作流**：它吐出来的是一份报告 + 一份可以直接贴进
 * `comfy_submit(overrides=…)` 的键值。真正动手的永远是那次提交（还要过审批与提交前预检）。
 * 于是"AI 自作主张把用户的模型换掉了"在结构上不可能发生。
 *
 * ## 四种结论，一种都不许含糊
 *
 * | 结论 | 含义 | 能不能直接写进 overrides |
 * | --- | --- | --- |
 * | [Status.EXACT] | 本机有同名文件**且当前实例能加载** | ✅ 唯一可以自动绑的一档 |
 * | [Status.ON_DISK_ONLY] | 盘上有同名，但**这个实例枚举不到**（模型根没挂全） | ❌ 先重启 / 修挂载，**别下载** |
 * | [Status.SUBSTITUTE] | 本机有更接近的（字面相似度 ≥ 阈值） | ❌ **先问用户**（那是他的模型） |
 * | [Status.MISSING] | 本机没有 | ❌ 去 `find_model` / `civitai_search` 查，**不许编文件名** |
 * | [Status.UNCHECKED] | 拿不到本机模型清单，没验证过 | ⚠ 只有 `force=true` 才写（如实标"没验证"） |
 * | [Status.NOT_IN_RECIPE] | 这条配方里压根没有这个角色 / 这个键 | ❌ 报出它的槽位清单 |
 * | [Status.AMBIGUOUS] | 同一个角色有多个槽位（两条 LoRA），说不清绑哪个 | ❌ 让调用方点明 `overrideKey` |
 *
 * `force=true` 是"我知道我在做什么"的通道（模型要换成用户自己指名的那个文件）：
 * 除了 [Status.NOT_IN_RECIPE] / [Status.AMBIGUOUS] 这两种**结构上写不出来**的，
 * 其余一律照写并附警告 —— 最后的守门人仍然是 `comfy_submit` 的**提交前预检**
 * （它会在真正要跑的那张图上再查一遍"这个实例加载不加载得了"）。
 *
 * ## 骨架接不上时要说清楚（设计 §3.1 情形 B）
 *
 * 常见的一种：配方是**分离式加载**（`diffusion_models` + `text_encoders` + `vae`），
 * 而用户给了一个**整包 checkpoint**。这不是"改个文件名"能解决的（要增删节点 + 重连线）——
 * 报告里会写明"这条骨架接不上，去找分离式版本 / 换一条用 checkpoint 的骨架"，
 * **不许**糊一个看着像的覆盖键上去。
 */
object WorkflowRecipeBinder {

    /** 一条候选最多给几个（与 `ComfyModels.MAX_CANDIDATES` 同量级）。 */
    const val MAX_CANDIDATES = 3

    /** 报告里最多带几条旋钮键（提示词 / 采样 / 尺寸）。 */
    const val MAX_KNOB_HINTS = 12

    /**
     * 一次绑定请求：`fileName` 必填，定位槽位用 `overrideKey`（最准）或 `role`（按角色）。
     *
     * 两者都不给时按**清单里的目录**推角色（"这个文件在哪类目录里，就绑这一类槽位"）——
     * 推不出来就如实报 [Status.NOT_IN_RECIPE] 并列出槽位清单（**不猜**）。
     */
    data class Request(val fileName: String, val role: String? = null, val overrideKey: String? = null)

    enum class Status(val wire: String) {
        EXACT("exact"),
        ON_DISK_ONLY("onDiskOnly"),
        SUBSTITUTE("substitute"),
        MISSING("missing"),
        UNCHECKED("unchecked"),

        /**
         * 这个名字**不是模型本身**，而是 ComfyUI / 工具链写在旁边的 sidecar。
         *
         * 真机探针（2026-09-26）抓到的：磁盘兜底清单里赫然列着
         * `anima-base-v1.0.metadata.json` —— 那是 ComfyUI（或模型下载器）给同名
         * `.safetensors` 写的元数据，`ComfyModels.MODEL_EXTS` 里有 `json`，于是它被当成一个模型。
         * 绑定时必须挡住：把一个 `.metadata.json` 写进 `ckpt_name`，ComfyUI 只会报"文件不存在"，
         * 而模型**明明就在盘上**（只是名字被 sidecar 顶掉了）。
         */
        NOT_A_MODEL("notAModel"),

        NOT_IN_RECIPE("notInRecipe"),
        AMBIGUOUS("ambiguous"),
    }

    /** 一条绑定的结论。 */
    data class Binding(
        val overrideKey: String?,
        val role: String?,
        val from: String?,
        val to: String,
        val status: Status,
        /** 人话：为什么是这个结论、下一步该做什么 */
        val note: String,
        val candidates: List<String> = emptyList(),
    ) {
        /** 这条绑定能不能直接写进 overrides。 */
        val applied: Boolean get() = overrideKey != null
    }

    data class Report(
        val recipeId: Long?,
        val recipeName: String,
        val task: String,
        val skeletonHash: String,
        /** 整条配方是不是"可以直接跑"（所有请求都 EXACT，且没有缺件） */
        val ready: Boolean,
        val overrides: Map<String, String>,
        val bindings: List<Binding>,
        /** 没被这次请求动过的槽位（保持工作流里的原值） */
        val untouched: List<String>,
        /** 可覆盖的旋钮键（提示词 / 采样 / 尺寸），给模型拼下一次提交用 */
        val knobHints: List<String>,
        val hints: List<String>,
        val warnings: List<String>,
    ) {
        fun toJson(): JsonObject = buildJsonObject {
            recipeId?.let { put("recipeId", it) }
            put("recipeName", recipeName)
            put("task", task)
            put("skeletonHash", skeletonHash)
            put("ready", ready)
            put(
                "overrides",
                buildJsonObject { overrides.forEach { (k, v) -> put(k, v) } },
            )
            put(
                "bindings",
                buildJsonArray {
                    bindings.forEach { b ->
                        add(
                            buildJsonObject {
                                b.overrideKey?.let { put("overrideKey", it) }
                                b.role?.let { put("role", it) }
                                b.from?.let { put("from", it) }
                                put("to", b.to)
                                put("status", b.status.wire)
                                put("applied", b.applied)
                                put("note", b.note)
                                if (b.candidates.isNotEmpty()) {
                                    put("candidates", buildJsonArray { b.candidates.forEach { add(JsonPrimitive(it)) } })
                                }
                            },
                        )
                    }
                },
            )
            put("untouched", buildJsonArray { untouched.forEach { add(JsonPrimitive(it)) } })
            put("knobHints", buildJsonArray { knobHints.forEach { add(JsonPrimitive(it)) } })
            put("hints", buildJsonArray { hints.forEach { add(JsonPrimitive(it)) } })
            put("warnings", buildJsonArray { warnings.forEach { add(JsonPrimitive(it)) } })
        }

        /** 给模型看的正文（与 JSON 说的是同一件事，只是排成人话）。 */
        fun render(): String = buildString {
            append("【把配方绑到模型上】").append(recipeName)
            append("　任务：").append(task).append("　骨架：").append(skeletonHash)
            append(if (ready) "　→ 可以直接跑\n" else "　→ **还不能直接跑**\n")
            bindings.forEach { b ->
                append("  · ").append(b.to)
                append(" [").append(b.status.wire).append("] ")
                b.overrideKey?.let { append("→ ").append(it).append(" ") }
                b.role?.let { append("（").append(it).append("）") }
                append("：").append(b.note).append('\n')
                if (b.candidates.isNotEmpty()) append("      候选：").append(b.candidates.joinToString("、")).append('\n')
            }
            if (overrides.isNotEmpty()) {
                append("要写进 comfy_submit 的 overrides：")
                append(overrides.entries.joinToString("、") { "${it.key}=${it.value}" })
                append('\n')
            } else {
                append("这次**一个键都不能改**（结论见上）\n")
            }
            if (untouched.isNotEmpty()) append("没动的槽位：").append(untouched.joinToString("、")).append('\n')
            if (knobHints.isNotEmpty()) append("还能覆盖的旋钮：").append(knobHints.joinToString("、")).append('\n')
            hints.forEach { append("→ ").append(it).append('\n') }
            warnings.forEach { append("⚠ ").append(it).append('\n') }
        }.trimEnd()
    }

    /**
     * 绑一次。
     *
     * @param inventory 本机模型清单；null = 拿不到（结论一律 [Status.UNCHECKED]，只有 `force` 才写键）
     * @param force 调用方**指名的文件**是否照写（见文件头那一段）
     */
    fun bind(
        recipe: WorkflowRecipe.Recipe,
        requests: List<Request>,
        inventory: ComfyModels.Inventory?,
        recipeId: Long? = null,
        recipeName: String = "（未命名）",
        complete: Boolean = true,
        force: Boolean = false,
    ): Report {
        val bindings = mutableListOf<Binding>()
        val overrides = LinkedHashMap<String, String>()
        val usedKeys = mutableSetOf<String>()
        val warnings = mutableListOf<String>()

        requests.forEach { req ->
            val target = resolveSlot(recipe, req, inventory)
            when (target) {
                is SlotLookup.NotFound -> bindings += Binding(
                    overrideKey = null,
                    role = req.role,
                    from = null,
                    to = req.fileName,
                    status = Status.NOT_IN_RECIPE,
                    note = target.note,
                )
                is SlotLookup.Ambiguous -> bindings += Binding(
                    overrideKey = null,
                    role = target.role,
                    from = null,
                    to = req.fileName,
                    status = Status.AMBIGUOUS,
                    note = "这个角色在配方里有 ${target.keys.size} 个槽位（${target.keys.joinToString("、")}）：" +
                        "说不清该绑哪个 —— 用 overrideKey 点明一个（例如 overrideKey=\"${target.keys.first()}\"），" +
                        "或者分多次绑。**不要**把一个文件写给两个槽位。",
                )
                is SlotLookup.One -> {
                    val slot = target.slot
                    val verdict = verify(req.fileName, slot.role, inventory)
                    if (target.assumed) {
                        warnings += "没说是哪一类槽位：我按**底模**那一格（${slot.overrideKey}）来对。" +
                            "要绑的是别的角色（LoRA / VAE / 文本编码器）就带 role 或 overrideKey 重来。"
                    }
                    // 只有两档写得出覆盖键：**同名且实例可见**（唯一能自动绑的一档），
                    // 或者调用方显式 `force`（"我知道我在做什么"，最后仍由提交前预检守门）
                    val canApply = verdict.status == Status.EXACT || force
                    val binding = Binding(
                        overrideKey = if (canApply) slot.overrideKey else null,
                        role = slot.role,
                        from = slot.fileName,
                        to = req.fileName,
                        status = verdict.status,
                        note = verdict.note,
                        candidates = verdict.candidates,
                    )
                    bindings += binding
                    if (binding.applied) {
                        overrides[slot.overrideKey] = req.fileName
                        usedKeys += slot.overrideKey
                    }
                    if (verdict.status == Status.ON_DISK_ONLY) {
                        warnings += "${req.fileName} 盘上有、但**当前实例枚举不到**：改了同样报文件不存在 —— " +
                            "先让用户重启 ComfyUI / 修好模型根（**不要下载**）。"
                    }
                    if (force && verdict.status != Status.EXACT && binding.applied) {
                        warnings += "${slot.overrideKey} 写的是**没验证过**的 ${req.fileName}（你要求 force）—— " +
                            "提交前预检会再查一次能不能加载。"
                    }
                }
            }
        }

        val untouched = recipe.slots.filter { it.overrideKey !in usedKeys }
            .map { "${it.overrideKey}（${it.role}）= ${it.fileName}" }
        val knobHints = recipe.knobs
            .filter {
                it.kind == WorkflowRecipe.KnobKind.PROMPT || it.kind == WorkflowRecipe.KnobKind.SAMPLING ||
                    it.kind == WorkflowRecipe.KnobKind.SEED || it.kind == WorkflowRecipe.KnobKind.SIZE ||
                    it.kind == WorkflowRecipe.KnobKind.IMAGE
            }
            .map { it.overrideKey }
            .distinct()
            .take(MAX_KNOB_HINTS)

        val ready = bindings.isNotEmpty() && bindings.all { it.status == Status.EXACT } && complete
        val hints = mutableListOf<String>()
        if (!complete) {
            hints += "这条配方 `complete=false`（学的时候这张图就缺件，见它的 gaps）：" +
                "**先把缺的节点类补齐**（本机实例装/启用那几个自定义节点）再来绑，别拿缺件的骨架开跑。"
        }
        if (bindings.any { !it.applied }) {
            hints += "先按上面的结论处理（问用户 / 重启实例 / 去找模型），**处理完再提交**；" +
                "现在带着这些键去提交会被提交前预检拦下来。"
        }
        if (recipe.slots.isEmpty()) {
            hints += "这条配方一个模型槽位都没认出来（可能是自定义加载器或纯 CPU 节点）：" +
                "换一条骨架，或者用 comfy_load_workflow + 手写 overrides。"
        }
        hints += "绑完用 comfy_submit(promptId=…, overrides={${overrides.entries.joinToString("、") { "\"${it.key}\":\"${it.value}\"" }}}) 提交；" +
            "promptId 从 comfy_load_workflow(path=…) 拿（配方里那条源文件与它是同一把钥匙）。"
        if (recipe.slots.any { it.roleGuessed }) {
            warnings += "这条配方里有槽位的**目录名是猜的**（自定义加载器）：覆盖键是准的，目录名别当结论。"
        }

        return Report(
            recipeId = recipeId,
            recipeName = recipeName,
            task = recipe.task.wire,
            skeletonHash = recipe.skeletonHash,
            ready = ready,
            overrides = overrides,
            bindings = bindings,
            untouched = untouched,
            knobHints = knobHints,
            hints = hints,
            warnings = warnings,
        )
    }

    // -----------------------------------------------------------------------
    //  内部
    // -----------------------------------------------------------------------

    private sealed interface SlotLookup {
        data class One(val slot: WorkflowRecipe.Slot, val assumed: Boolean = false) : SlotLookup
        data class Ambiguous(val role: String, val keys: List<String>) : SlotLookup
        data class NotFound(val note: String) : SlotLookup
    }

    /**
     * "底模那一格"：`checkpoints` 或 `diffusion_models`。
     *
     * 参数名叫 `model`、用户说的也是"换成这个底模"，所以**没说角色**时按它兜底 ——
     * 但兜底要**说出来**（[SlotLookup.One.assumed] → 报告里一条警告），
     * 不能让人以为"它自己知道我指的是哪一格"。
     */
    private val BASE_ROLES = setOf("checkpoints", "diffusion_models")

    /** 槽位清单（人话），`NotFound` / `Ambiguous` 的提示里要用。 */
    private fun slotList(recipe: WorkflowRecipe.Recipe): String =
        if (recipe.slots.isEmpty()) "（一个都没有）"
        else recipe.slots.joinToString("、") { "${it.overrideKey}（${it.role}）= ${it.fileName}" }

    private fun resolveSlot(
        recipe: WorkflowRecipe.Recipe,
        req: Request,
        inventory: ComfyModels.Inventory?,
    ): SlotLookup {
        req.overrideKey?.let { key ->
            val slot = recipe.slots.firstOrNull { it.overrideKey == key }
            return if (slot != null) SlotLookup.One(slot)
            else SlotLookup.NotFound(
                "这条配方里没有 `$key` 这个槽位。它的槽位是：${slotList(recipe)}。" +
                    "（覆盖键写成 `节点id.输入名`；不确定就先用 comfy_list_recipes 看配方正文。）",
            )
        }
        req.role?.let { role ->
            val slots = recipe.slots.filter { it.role.equals(role, ignoreCase = true) }
            return when {
                slots.isEmpty() -> SlotLookup.NotFound(
                    "这条配方里没有 role=$role 的槽位。它的槽位是：${slotList(recipe)}。",
                )
                slots.size == 1 -> SlotLookup.One(slots[0])
                else -> SlotLookup.Ambiguous(role, slots.map { it.overrideKey })
            }
        }
        // 两者都没给：按清单里的**目录**推角色（推不出来再退回"底模那一格"）
        val found = inventory?.models?.firstOrNull { ModelName.same(it.name, req.fileName) }
        if (found != null) {
            val slots = recipe.slots.filter { it.role.equals(found.folder, ignoreCase = true) }
            return when {
                slots.size == 1 -> SlotLookup.One(slots[0])
                slots.size > 1 -> SlotLookup.Ambiguous(found.folder, slots.map { it.overrideKey })
                else -> SlotLookup.NotFound(mismatchNote(recipe, found.folder, req.fileName))
            }
        }
        // 清单里没有这个文件（或根本没有清单）：按"底模那一格"兜底 —— 参数名叫 model、
        // 用户说的也是"换成这个底模"。兜底**要标出来**，见 BASE_ROLES 的注释。
        val base = recipe.slots.filter { it.role in BASE_ROLES }
        if (base.size == 1) return SlotLookup.One(base[0], assumed = true)
        return SlotLookup.NotFound(
            "没说绑哪个槽位（既没有 role 也没有 overrideKey），而这个文件名在本机清单里也找不到，" +
                "所以我不知道它属于哪一类。这条配方的槽位是：${slotList(recipe)}。" +
                "要么带上 role（${recipe.slots.map { it.role }.distinct().joinToString(" / ")}），" +
                "要么带上 overrideKey。",
        )
    }

    /**
     * "目录对不上"时的那句解释（设计 §3.1 情形 B）。
     *
     * 最常见的两种：整包 checkpoint ↔ 分离式 unet+clip+vae；LoRA 被放进 checkpoints。
     * 这两种都不是"改个文件名"能解决的，必须让模型知道**该换个方向**。
     */
    private fun mismatchNote(recipe: WorkflowRecipe.Recipe, requestedFolder: String, fileName: String): String {
        val roles = recipe.slots.map { it.role }.distinct()
        val base = "这条配方里没有 `$requestedFolder` 这一类槽位（它有：${roles.joinToString("、")}）。"
        return when {
            requestedFolder == "checkpoints" && roles.contains("diffusion_models") ->
                base + " 它用的是**分离式加载**（底模 + 文本编码器 + VAE 三个槽位），" +
                    "而你给的是一个**整包 checkpoint** —— 覆盖键换不上去：要真接上得**改写骨架**" +
                    "（增删节点 + 重连线，本轮不做）。两条路：① 找这个模型的**分离式**版本" +
                    "（unet / text encoder / vae 分开的）；② 换一条**用 checkpoint 的**同任务骨架" +
                    "（`comfy_list_recipes(task=…)` 里找）。"
            requestedFolder == "diffusion_models" && roles.contains("checkpoints") ->
                base + " 它用的是**整包 checkpoint**，而你给的是一个裸 unet / DiT。" +
                    "分离式加载要自己补文本编码器与 VAE 两个节点（本轮不做骨架改写）：" +
                    "① 找这个模型的整包版本；② 换一条用 unet 的同任务骨架。"
            requestedFolder == "loras" && !roles.contains("loras") ->
                base + " LoRA 只能挂在 LoRA 槽位上：这条骨架没有 LoRA 位置（想加得改骨架）。"
            else -> base + " 换一条有这一类槽位的骨架，或者用 overrideKey 直接指一个已存在的槽位" +
                "（**不要**把一个 $requestedFolder 的文件写进别的目录的槽位）。"
        }
    }

    private data class Verdict(val status: Status, val note: String, val candidates: List<String> = emptyList())

    /**
     * 这是不是"模型旁边的元数据"而不是模型本身。
     *
     * 只认两种**确定**的形状（宁可漏判，也不要把真模型当成 sidecar 挡掉）：
     * `*.metadata.json`（ComfyUI / 下载器写的同名元数据）与 `model_index.json`
     * （diffusers 那种"目录形态的模型"标记文件）。
     */
    private fun looksLikeSidecar(name: String): Boolean {
        val lower = name.lowercase().replace('\\', '/')
        val base = lower.substringAfterLast('/')
        return base.endsWith(".metadata.json") || base == "model_index.json"
    }

    /**
     * 这个文件名在本机能不能用（四档 + 没有清单时的 `unchecked`）。
     *
     * 判据与 `WorkflowFit.fit` **同源**：同名且实例可见才算 `exact`；
     * 盘上有同名但实例看不到 → `onDiskOnly`（先重启/修挂载，别下载）。
     */
    private fun verify(fileName: String, role: String, inventory: ComfyModels.Inventory?): Verdict {
        if (looksLikeSidecar(fileName)) {
            return Verdict(
                Status.NOT_A_MODEL,
                "「$fileName」看着是 ComfyUI / 下载器写在模型旁边的 **sidecar 元数据**（不是模型本身）：" +
                    "真正的模型是同名的 `…safetensors` —— 用那个名字。",
            )
        }
        if (inventory == null) {
            return Verdict(
                Status.UNCHECKED,
                "拿不到本机模型清单（ComfyUI 没在跑 / 没找到 models 目录），**这个名字没验证过**：" +
                    "要照写就带 force=true，我会把它当'你指名要的文件'写进覆盖键（提交前预检还会再查一次）。",
            )
        }
        // sidecar 一律不进候选（见 Status.NOT_A_MODEL 的注释：它会把真模型顶掉）
        val candidates = inventory.candidatesFor(role, fileName).filterNot { looksLikeSidecar(it.model.name) }
        val exactVisible = candidates.firstOrNull { ModelName.same(fileName, it.model.name) && it.model.visibleToInstance }
        if (exactVisible != null) {
            return Verdict(Status.EXACT, "本机有同名文件，当前实例能加载 —— 直接绑。")
        }
        val exactAny = candidates.firstOrNull { ModelName.same(fileName, it.model.name) }
        if (exactAny != null) {
            return Verdict(
                Status.ON_DISK_ONLY,
                "盘上有同名文件（${exactAny.model.path ?: exactAny.model.name}），但**当前这个实例枚举不到它**：" +
                    "说明实例的模型根没挂全 —— 先重启 ComfyUI / 修好挂载，**不要下载**，也不要硬写这个键。",
                candidates.take(MAX_CANDIDATES).map { it.model.name },
            )
        }
        if (candidates.isNotEmpty()) {
            return Verdict(
                Status.SUBSTITUTE,
                "本机没有同名的；下面这些是**字面相似**的候选（相似度只是词根对得上，**不代表架构兼容**）：" +
                    "换之前**先问用户**（那是他的模型）。",
                candidates.take(MAX_CANDIDATES).map { "${it.model.name}（${"%.2f".format(it.score)}${it.model.sizeGb?.let { g -> "，%.1fGB".format(g) } ?: ""}）" },
            )
        }
        val near = inventory.nearMissesFor(role, fileName).map { it.name }.filterNot { looksLikeSidecar(it) }
        val nearText = if (near.isEmpty()) "" else "（有几个名字沾边的：${near.joinToString("、")}，但都不够像）"
        return Verdict(
            Status.MISSING,
            "本机没有这个文件$nearText：**绝对不要自己编一个文件名**。先去 `find_model` / `civitai_search` 查它，" +
                "再用 `ask_user_question` 让用户决定装 / 换骨架 / 放弃。",
            near,
        )
    }

    /** 报告里"这条配方有哪些槽位"的紧凑形状（工具列表用）。 */
    fun slotsJson(recipe: WorkflowRecipe.Recipe): JsonArray = buildJsonArray {
        recipe.slots.forEach { s ->
            add(
                buildJsonObject {
                    put("overrideKey", s.overrideKey)
                    put("role", s.role)
                    put("fileName", s.fileName)
                    put("roleGuessed", s.roleGuessed)
                },
            )
        }
    }
}
