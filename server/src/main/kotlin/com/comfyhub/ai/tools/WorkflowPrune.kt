package com.comfyhub.ai.tools

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * 工作流的**摘节点 / 拼节点**（用户建议 ②，2026-09-20）。
 *
 * ## 用户的原话
 *
 * > "可能要设计工具/skill，让 AI 能够基于基本工作流，摘除无关节点或者拼装必要节点
 * > （比如这种情况：用户没有提供参考图，但是他引导的工作流的节点用到参考图）"
 *
 * 后来用户又补了两个现场（2026-09-20 的对话）：
 * > "我没给参考图，但是给了个需要参考图的工作流，要能识别出该工作流的骨干部分，
 * >  把无关内容临时摘掉；又比如我显存不足但是硬要跑，这时候可能需要考虑向工作流加入
 * >  Sage Attention"
 *
 * ## 为什么这件事必须由后端**确定性地**做
 *
 * "摘掉一个分支"不是删几个节点就完事：下游那些指着它的连线要**重新接回去**
 * （`KSampler.positive` 原本接在 ControlNet 上，摘掉 ControlNet 之后得接回文本编码器）。
 * 让模型拿 JSON 自己重写，它得先把整张图读进来、再逐条重写引用 —— 32K 字符的结果上限
 * 装不下一份 100+ 节点的工作流，而且一个键写错 ComfyUI 只会回一句"节点不存在"。
 * 所以这里做的是纯函数式的图运算，结果**自带审计清单**（摘了谁、把哪根线接到了哪），
 * 模型与用户都能核对。
 *
 * ## 判据是"**可达性**"，不是"节点名字像不像"
 *
 * 摘除的做法只有一条：从**输出节点**（SaveImage / PreviewImage / SaveVideo…）出发
 * 反向遍历，能走到的才是骨干。剩下的一律是"跟出图无关的分支" —— 不靠类名白名单去猜
 * "这个像不像 ControlNet"，所以第三方自定义节点也照样管用。
 *
 * ## 两个动作
 *
 *  - **摘除**：`remove` 点名要删的输入（`89.image`）+ 它们所在的分支；
 *  - **拼装**：`inject` 把"必须补上"的节点插进链里（典型是 Sage Attention /
 *    显存优化节点：插在模型加载器与采样器之间的 `model` 线上）。
 *
 * 拼装用的是**节点定义表**（ComfyUI 的 `/object_info`）：每种补丁节点要接哪些输入、
 * 从哪接、接到哪，都按定义算，不靠硬编码某台机器的节点名。
 */
object WorkflowPrune {

    /**
     * 输出节点：从这些出发做反向可达性分析。
     *
     * 判据与 [WorkflowInputs] 那边一致（"Save / Preview / VideoCombine 是结果去哪儿"），
     * 这里是它的对偶用法。
     */
    private val OUTPUT_HINTS = listOf("save", "preview", "videocombine", "export")

    /** 这些类不是"出图链上的东西"，但摘掉会让图跑不出来 —— 不算无关分支。 */
    private val ALWAYS_KEEP_CONDITION = { cls: String ->
        cls.contains("Sampler", true) || cls.contains("Guider", true) ||
            cls.contains("Scheduler", true) || cls.contains("Noise", true)
    }

    /**
     * 一次摘除 / 拼装的结果。
     *
     * [graph] 是**改好的新图**（原图不动，见"写入留退路"那条纪律）；
     * 其余字段是给模型与用户核对用的审计清单。
     */
    data class Result(
        val graph: JsonObject,
        /** 摘掉了哪些节点，形如 `89 LoadImage` */
        val removed: List<String>,
        /** 被重新接过的线，形如 `3.positive ← 6（原来接在 89 上）` */
        val rewired: List<String>,
        /** 插进去的节点，形如 `900 SageAttention（插在 3.model 上）` */
        val injected: List<String>,
        /** 摘完之后**还悬着**的输入（这些会让 ComfyUI 报缺输入，必须如实报出来） */
        val unresolved: List<String>,
        /** 摘完之后空着的连线型输入（模型可以照它补线） */
        val openInputs: List<String>,
        /** 每个输出节点还连不连得通（false = 这份图跑不出东西了） */
        val outputsAlive: Int,
        val outputsTotal: Int,
        /** 干不了 / 不敢干的说明（不静默） */
        val warnings: List<String> = emptyList(),
    ) {
        val runnable: Boolean
            get() = unresolved.isEmpty() && outputsAlive > 0 && outputsAlive == outputsTotal

        fun summary(): String = buildString {
            append("摘掉 ").append(removed.size).append(" 个节点")
            if (rewired.isNotEmpty()) append("，重接 ").append(rewired.size).append(" 根线")
            if (injected.isNotEmpty()) append("，插入 ").append(injected.size).append(" 个节点")
            append("；输出节点 ").append(outputsAlive).append('/').append(outputsTotal).append(" 仍然连通")
            if (unresolved.isNotEmpty()) append("；**还有 ").append(unresolved.size).append(" 处悬空**")
            if (warnings.isNotEmpty()) append("；").append(warnings.size).append(" 条提醒")
        }
    }

    /** 拼装补丁的声明：插在哪条线、节点定义名、以及它自己的输入从哪来。 */
    data class Injection(
        /** 要插进哪个输入：`节点id.输入名`（例如 `3.model`） */
        val at: String,
        /** 节点类名（必须在 `/object_info` 里有定义，或者是个纯字面量节点） */
        val classType: String,
        /** 这个补丁节点自己的输入：名字 → 值（`"model"` 这类"接上游"的输入自动接） */
        val inputs: Map<String, JsonElement> = emptyMap(),
        /** 新节点用什么编号（默认自动取一个没被占用的） */
        val nodeId: String? = null,
    )

    /**
     * 摘 + 拼。
     *
     * @param graph API 格式节点图（`{"1":{"class_type":"…","inputs":{…}}}`）。
     * @param removeFrom 要摘掉的输入（`89.image`）或节点 id（`89`）——
     *   摘的是**它们所在的那条分支**，不是只摘那一个节点。
     * @param keep 明确要求保留的节点 id（摘分歧时防止误伤）。
     * @param injections 要拼进去的节点（典型：Sage Attention）。
     * @param objectInfo ComfyUI 的节点定义表；给了就能算出"补丁节点要接哪些输入"。
     *   为 null 时只做摘除（拼装需要定义表才不会接错线）。
     */
    fun apply(
        graph: JsonObject,
        removeFrom: List<String> = emptyList(),
        keep: Set<String> = emptySet(),
        injections: List<Injection> = emptyList(),
        objectInfo: JsonObject? = null,
    ): Result {
        val nodes = LinkedHashMap<String, JsonObject>()
        graph.forEach { (id, v) -> (v as? JsonObject)?.let { nodes[id] = it } }
        if (nodes.isEmpty()) {
            return Result(
                graph = graph,
                removed = emptyList(),
                rewired = emptyList(),
                injected = emptyList(),
                unresolved = emptyList(),
                openInputs = emptyList(),
                outputsAlive = 0,
                outputsTotal = 0,
                warnings = listOf("这张图里一个节点都没有（不是 API 格式节点图？）"),
            )
        }

        val warnings = mutableListOf<String>()
        val outputs = nodes.filterKeys { isOutput(it, nodes) }.keys.toList()

        // ---- 1) 摘除 ------------------------------------------------------
        //
        // 用户点的那个位置**一定**要摘掉，摘完之后：
        //   ① 它自己的连线输入清空（= 这条线不再存在）；
        //   ② 由此"没人再要"的上游跟着级联摘掉（`LoadImage` / `ControlNetLoader`
        //      只喂给 `ControlNetApply`，ControlNetApply 一走它们就再也不会被执行）；
        //   ③ 它下游那些**还有消费者的**人不摘，而是把指着它的线**接回它自己的上游**
        //      （`KSampler.positive` 就是这么回到文本编码器上的）。
        val dropped = LinkedHashSet<String>()
        val targets = LinkedHashSet<String>()
        removeFrom.forEach { raw ->
            val target = raw.trim()
            if (target.isEmpty()) return@forEach
            val nodeId = if (target.contains('.')) target.substringBefore('.') else target
            if (nodeId !in nodes) {
                warnings += "要摘的节点 $nodeId 不在这张图里（现有节点 ${nodes.keys.take(20).joinToString("、")}…）"
                return@forEach
            }
            if (nodeId in outputs) {
                warnings += "节点 $nodeId（${nodes[nodeId]?.cls()}）是**输出节点**，摘了什么图都出不来 —— 已保留"
                return@forEach
            }
            if (nodeId in keep) {
                warnings += "节点 $nodeId（${nodes[nodeId]?.cls()}）在 keep 里，已保留"
                return@forEach
            }
            targets += nodeId
            dropped += nodeId
        }

        // 级联：把"**再也不会被执行**"的节点收掉。
        //
        // 判据只有一条，好解释也好验证：**从输出节点反向再也走不到它** ——
        // 它已经不在这张图的出图链路里了（`LoadImage` / `ControlNetApply` /
        // `ControlNetLoader` 都是这么走的）。
        //
        // 两处例外必须留着，否则会变成"顺手删用户的图"：
        //   · **输出节点**永远保留；
        //   · **没有任何连线输入、也没有消费者**的孤立节点不碰 ——
        //     那是用户"还没接完"的东西（比如刚拖进来的 EmptyLatentImage），
        //     不是我们要摘的那条无关分支。第一版就是把它一起删了。
        var cascading = true
        while (cascading) {
            cascading = false
            val live = liveNodes(nodes.filterKeys { it !in dropped }, outputs)
            nodes.forEach { (id, node) ->
                if (id in dropped || id in keep || id in outputs || id in live) return@forEach
                val inputs = (node["inputs"] as? JsonObject).orEmpty()
                val wired = inputs.values.any { refOf(it) != null }
                val hasConsumers = consumersOf(id, nodes, dropped).isNotEmpty()
                if (!wired && !hasConsumers) return@forEach // 孤立节点：不碰
                dropped += id
                cascading = true
            }
        }

        val kept = LinkedHashMap(nodes.filterKeys { it !in dropped })

        // ---- 2) 收拾残局：指着被摘节点的输入，接回它自己的上游 --------------
        //
        // 这一段是"用户还看得见结果"的关键：
        //  · 目标本身的输入一律清掉（`KSampler.positive` 不再接 ControlNet）；
        //  · **只是**把目标当上游、且还有别的连线输入的人，改接回目标的上游
        //    （`ControlNetApply` 摘下之后，它下游那条链不至于断在半空）。
        val rewired = mutableListOf<String>()
        val patched = LinkedHashMap<String, JsonObject>()
        kept.forEach { (id, node) ->
            val inputs = (node["inputs"] as? JsonObject)?.toMutableMap() ?: mutableMapOf()
            var changed = false
            for (name in inputs.keys.toList()) {
                val ref = refOf(inputs[name]) ?: continue
                if (ref.first !in dropped) continue
                if (id in targets) {
                    // 目标自己的输入：清掉（这就是"把这条线拿走"）
                    inputs.remove(name)
                    changed = true
                    continue
                }
                // 别人指着被摘节点：接回它的上游（找不到就置空，并如实报出来）
                val back = nearestLiveInput(ref.first, nodes, dropped)
                if (back == null) {
                    inputs.remove(name)
                    warnings += "$id.$name 原本接在被摘掉的 ${ref.first} 上，" +
                        "而 ${ref.first} 也没有可用的上游 → 这个输入现在空着"
                } else {
                    inputs[name] = JsonArray(listOf(JsonPrimitive(back.first), JsonPrimitive(back.second)))
                    rewired += "$id.$name ← ${back.first}（原来接在被摘掉的 ${ref.first} 上）"
                }
                changed = true
            }
            patched[id] = if (changed) JsonObject(node + mapOf("inputs" to JsonObject(inputs))) else node
        }

        // ---- 3) 拼装：插补丁节点 ------------------------------------------
        val injected = mutableListOf<String>()
        var nextId = (patched.keys.mapNotNull { it.toLongOrNull() }.maxOrNull() ?: 0L) + 100L
        injections.forEach { inj ->
            val (hostId, hostInput) = splitPath(inj.at)
                ?: run {
                    warnings += "拼装位置要写成 节点id.输入名（收到的是「${inj.at}」）"
                    return@forEach
                }
            val host = patched[hostId] as? JsonObject
            if (host == null) {
                warnings += "拼装位置 $hostId 不在这张图里（可能刚好被摘掉了）"
                return@forEach
            }
            val hostInputs = (host["inputs"] as? JsonObject)?.toMutableMap() ?: mutableMapOf()
            val upstream = hostInputs[hostInput]
            val upstreamRef = refOf(upstream)
            if (upstreamRef == null) {
                warnings += "$hostId.$hostInput 不是一条线（现在值：${upstream ?: "空"}），" +
                    "没法把 ${inj.classType} 插在它中间"
                return@forEach
            }
            if (objectInfo != null && objectInfo[inj.classType] == null) {
                warnings += "这台 ComfyUI 里没有节点类 ${inj.classType}（/object_info 查不到）——没有插入"
                return@forEach
            }
            val newId = inj.nodeId?.takeIf { it.isNotBlank() && !patched.containsKey(it) }
                ?: generateSequence { (nextId++).toString() }.first { !patched.containsKey(it) }

            val inputs = LinkedHashMap<String, JsonElement>()
            // 补丁自己的输入：显式给的先填
            inj.inputs.forEach { (k, v) -> inputs[k] = v }
            // 然后把它自己那条"接上游"的线接好：找出它声明的、类型与上游相同的那个输入
            val patchSpec = objectInfo?.get(inj.classType) as? JsonObject
            val carryOver = pickCarryInput(patchSpec, inj.classType, inputs)
            if (carryOver == null) {
                warnings += "不敢给 ${inj.classType} 自动接线：从定义里看不出哪个输入该接上游，" +
                    "插入位置保持原样。可以把它的输入用 injections[].inputs 显式写清楚。"
                return@forEach
            }
            inputs[carryOver] = upstream!!
            hostInputs[hostInput] = JsonArray(listOf(JsonPrimitive(newId), JsonPrimitive(0)))
            patched[hostId] = JsonObject(host + mapOf("inputs" to JsonObject(hostInputs)))
            patched[newId] = buildJsonObject {
                put("class_type", inj.classType)
                put("inputs", JsonObject(inputs))
                put("_meta", buildJsonObject { put("title", "${inj.classType}（自动插入）") })
            }
            injected += "$newId ${inj.classType}（插在 $hostId.$hostInput 上）"
            nextId = (newId.toLongOrNull() ?: nextId) + 1
        }

        // ---- 4) 体检：悬空引用 / 空着的连线型输入 / 输出还通不通 ----------
        val unresolved = dangling(patched)
        val openInputs = openWiredInputs(patched, objectInfo)
        val alive = outputs.count { reachable(it, patched) }

        return Result(
            graph = JsonObject(patched),
            removed = dropped.map { "$it ${nodes[it]?.cls().orEmpty()}" }.sorted(),
            rewired = rewired,
            injected = injected,
            unresolved = unresolved,
            openInputs = openInputs,
            outputsAlive = alive,
            outputsTotal = outputs.size,
            warnings = warnings,
        )
    }

    /**
     * 一份"这段是干什么的"人话描述（用户建议 ④：分析出本地**用不上**的节点，
     * 向用户确认后再删）。
     *
     * 判据是几条最常见的类名特征，**认不出就照抄类名** —— 编一个听起来很懂的解释
     * 比不解释更坏（用户会照着它做决定）。这一段只影响文案，不影响摘除判据。
     */
    fun describe(cls: String): String {
        val c = cls.lowercase()
        return when {
            c.contains("controlnet") -> "ControlNet（结构控制）"
            c.contains("ipadapter") -> "IPAdapter（参考图/风格）"
            c.contains("loadimage") -> "LoadImage（参考图文件）"
            c.contains("loadvideo") || c.contains("vhs_loadvideo") -> "LoadVideo（参考视频）"
            c.contains("loadaudio") -> "LoadAudio（参考音频）"
            c.contains("loraloader") || c.contains("lora_tag") -> "LoRA 加载"
            c.contains("vae") -> "VAE 加载/解码"
            c.contains("clip") -> "文本编码器加载"
            c.contains("unetloader") || c.contains("checkpointloader") ||
                c.contains("diffusionmodelloader") -> "模型加载"
            c.contains("upscale") || c.contains("imagescale") -> "放大 / 缩放"
            c.contains("faceswap") || c.contains("facerestore") -> "换脸 / 修脸"
            c.contains("detailer") || c.contains("adetailer") -> "细节修复（Detailer）"
            c.contains("segment") || c.contains("mask") -> "遮罩 / 分割"
            c.contains("pose") || c.contains("openpose") -> "姿态提取"
            c.contains("depth") -> "深度提取"
            c.contains("label") -> "Label（纯界面标注，不参与出图）"
            c.contains("note") || c.contains("bookmark") -> "笔记 / 书签（纯界面元素）"
            c.contains("reroute") -> "Reroute（纯转接线）"
            // 认不出来就照抄类名，别编
            else -> cls
        }
    }

    /**
     * **用不上的分支**分析（用户建议 ④，2026-09-22）。
     *
     * > "设计工具，当用户提出类似'删除无用节点'时，分析出本地用不上的节点并向用户再次确认删除，
     * >  然后执行操作。"
     *
     * 判据与摘除完全同源：**从输出节点反向走不到** = 跟出图无关。不去猜"这个类像不像
     * ControlNet"（那样第三方节点就漏了），所以结果和真摘一次是一致的。
     *
     * @param graph API 格式节点图
     * @param keep 用户点名要保留的节点（不会进建议）
     * @param maxBranches 最多报几条分支（一次报 40 段没人看得完）
     */
    fun unusedBranches(
        graph: JsonObject,
        keep: Set<String> = emptySet(),
        maxBranches: Int = 12,
    ): Report {
        val nodes = LinkedHashMap<String, JsonObject>()
        graph.forEach { (id, v) -> (v as? JsonObject)?.let { nodes[id] = it } }
        val outputs = nodes.filterKeys { isOutput(it, nodes) }.keys.toList()
        val live = liveNodes(nodes, outputs)

        // "还没接完"的孤立节点：**自己没接任何线，也没人用它**。
        //
        // ⚠ 两个条件缺一不可（写第一版时只判了"没接线"，于是把 `FaceSwap.face` 指着的
        // `LoadImage` 也算成了孤立 —— 它是这条死分支的上游，摘的时候要一起走）。
        // 判据与 `apply` 级联摘除里那条"孤立节点不碰"逐字一致。
        fun isolated(id: String): Boolean {
            if (id in live) return false
            if (consumersOf(id, nodes).isNotEmpty()) return false
            val inputs = (nodes[id]?.get("inputs") as? JsonObject).orEmpty()
            return inputs.values.none { refOf(it) != null }
        }

        val orphanIds = nodes.keys.filter { it !in live && isolated(it) }.toSet()
        val deadIds = nodes.keys.filter { it !in live && it !in orphanIds && it !in keep && it !in outputs }
        val deadSet = deadIds.toSet()

        val raw = deadIds.map { id ->
            Candidate(
                id = id,
                cls = nodes[id]?.cls().orEmpty(),
                usedBy = consumersOf(id, nodes).sorted(),
            )
        }

        // 连成一片的算一条分支 —— 但"连成一片"必须**按数据流方向**连，不是无向连通：
        // 一条分支 = 一个末端（谁也不引用的死节点）＋ 喂给它的所有上游。
        // 用户在图里点那个末端，整条分支就跟着被摘掉，这正是 `apply` 的级联判据。
        //
        // （第一版按无向连通分组，把 `FaceSwap.face ← LoadImage` 这种"横向喂料"也连进
        // 同一片，于是一片里出现两个互不喂料的梢，入口就选到了错误的那个。）
        val byTip = LinkedHashMap<String, MutableList<Candidate>>()
        raw.forEach { cand ->
            val tip = downstreamTip(cand.id, nodes, deadSet)
            byTip.getOrPut(tip) { mutableListOf() }.add(cand)
        }
        val branches = mutableListOf<Branch>()
        byTip.forEach { (tipId, group) ->
            val entry = group.firstOrNull { it.id == tipId }
                ?: group.maxByOrNull { collect(it.id, nodes).count { id -> id in deadSet } }
                ?: group.first()
            val classes = group.map { it.cls }.distinct()
            branches += Branch(
                index = branches.size + 1,
                entry = entry.id,
                entryCls = entry.cls,
                label = describe(entry.cls),
                nodes = group,
                kinds = classes.map { describe(it) }.distinct().take(6),
            )
        }

        val sorted = branches
            .sortedWith(compareByDescending<Branch> { it.nodes.size }.thenBy { it.entry.toLongOrNull() ?: Long.MAX_VALUE })
            .mapIndexed { i, b -> b.copy(index = i + 1) }
            .take(maxBranches)

        val shown = sorted.sumOf { it.nodes.size }
        return Report(
            branches = sorted,
            isolated = orphanIds.sortedBy { it.toLongOrNull() ?: Long.MAX_VALUE }
                .map { Candidate(it, nodes[it]?.cls().orEmpty(), emptyList()) },
            totalUnused = deadIds.size,
            totalIsolated = orphanIds.size,
            totalNodes = nodes.size,
            outputCount = outputs.size,
            truncated = deadIds.size > shown,
        )
    }

    /** 一条**用不上的分支**（连成一片、可以整条摘掉）。 */
    data class Branch(
        val index: Int,
        /** 建议的入口节点 id（`removeFrom` 就写它） */
        val entry: String,
        val entryCls: String,
        /** 这条分支在做什么（人话） */
        val label: String,
        val nodes: List<Candidate>,
        /** 组内出现过的节点种类（人话，最多 6 条） */
        val kinds: List<String>,
    ) {
        /**
         * 这条分支要摘的节点 id。
         *
         * **整条都给**（不是只给 entry）：`removeFrom` 只写入口时，`apply` 会按"从输出反向
         * 再也走不到"级联摘掉同一片 —— 结果是一样的，但显式列全了，用户核对时不会漏。
         */
        fun suggestRemove(): List<String> = nodes.map { it.id }
    }

    /** 一个候选节点。 */
    data class Candidate(
        val id: String,
        val cls: String,
        /** 图里还有谁在引用它（空 = 这条分支的末端） */
        val usedBy: List<String>,
    )

    /** 分析结果（只报不改）。 */
    data class Report(
        val branches: List<Branch>,
        /** 未接线（还没接完）的孤立节点 —— 明确**不建议**删 */
        val isolated: List<Candidate>,
        val totalUnused: Int,
        val totalIsolated: Int,
        val totalNodes: Int,
        val outputCount: Int,
        /** 超过 [unusedBranches] 的 maxBranches，还有没列出来的 */
        val truncated: Boolean,
    ) {
        fun toJson(): JsonObject = buildJsonObject {
            put("ok", true)
            put(
                "summary",
                if (totalUnused == 0) {
                    "这份工作流没有用不上的分支：$totalNodes 个节点全都在出图链路上。"
                } else {
                    "这份工作流共 $totalNodes 个节点，其中 **$totalUnused 个** 从任何一个输出节点" +
                        "（共 $outputCount 个）反向都走不到 —— 也就是跟出图无关，可以摘掉" +
                        "（分成 ${branches.size} 条分支）。"
                },
            )
            put("totalNodes", totalNodes)
            put("totalUnused", totalUnused)
            put("outputCount", outputCount)
            put("truncated", truncated)
            put(
                "branches",
                buildJsonArray {
                    branches.forEach { b ->
                        add(
                            buildJsonObject {
                                put("index", b.index)
                                put("removeFrom", b.entry)
                                put("label", b.label)
                                put("classType", b.entryCls)
                                put("nodeCount", b.nodes.size)
                                put(
                                    "nodes",
                                    buildJsonArray {
                                        b.nodes.forEach { n ->
                                            add(
                                                buildJsonObject {
                                                    put("id", n.id)
                                                    put("classType", n.cls)
                                                    put("label", describe(n.cls))
                                                    if (n.usedBy.isNotEmpty()) {
                                                        put("usedBy", JsonArray(n.usedBy.map { JsonPrimitive(it) }))
                                                    }
                                                }
                                            )
                                        }
                                    },
                                )
                                put("kinds", JsonArray(b.kinds.map { JsonPrimitive(it) }))
                            }
                        )
                    }
                },
            )
            if (isolated.isNotEmpty()) {
                put(
                    "isolated",
                    buildJsonArray {
                        isolated.forEach { n ->
                            add(
                                buildJsonObject {
                                    put("id", n.id)
                                    put("classType", n.cls)
                                    put("label", describe(n.cls))
                                }
                            )
                        }
                    },
                )
            }
            put(
                "hint",
                if (totalUnused == 0) {
                    "没有可摘的分支。别为了'看起来干净'去动还在链路里的节点。"
                } else {
                    "**先问用户再动手**（`ask_user_question`，选项用每条的 label）：" +
                        "把上面每条的 removeFrom / label / 节点数说清楚，用户确认哪几条，" +
                        "再用 `removeFrom=[那些 entry]` 调一次本工具（这次不带 analyze）真正执行。" +
                        "`isolated` 里的是**还没接完**的节点，不要删。"
                },
            )
        }
    }

    /**
     * [id] 这条死数据流最终喂给谁（**顺流而下**找那个谁也不引用的死节点）。
     *
     * 它就是"这条分支的梢"：用户在图里点它，`apply` 的级联会把整条一起摘掉。
     * 多个梢都能到（分叉到两个末端）时取编号大的那个，保证同一份图每次报同一条。
     */
    private fun downstreamTip(id: String, nodes: Map<String, JsonObject>, dead: Set<String>): String {
        var cursor = id
        var best = id
        val seen = mutableSetOf<String>()
        var guard = 0
        while (guard++ < 256 && seen.add(cursor)) {
            best = cursor
            val next = consumersOf(cursor, nodes).filter { it in dead }.maxByOrNull {
                it.toLongOrNull() ?: Long.MIN_VALUE
            } ?: break
            cursor = next
        }
        return best
    }

    /** 谁在用它（哪些节点把它当上游）；[ignoring] 里的节点不算（它们已经被决定摘掉了）。 */
    private fun consumersOf(
        nodeId: String,
        nodes: Map<String, JsonObject>,
        ignoring: Set<String> = emptySet(),
    ): Set<String> {
        val out = LinkedHashSet<String>()
        nodes.forEach { (id, node) ->
            if (id == nodeId || id in ignoring) return@forEach
            (node["inputs"] as? JsonObject)?.values?.forEach { v ->
                if (refOf(v)?.first == nodeId) out += id
            }
        }
        return out
    }

    /** 谁在用它（带输入名，删除时要一并清掉那些引用）。 */
    private fun consumersWithInputs(nodeId: String, nodes: Map<String, JsonObject>): List<Pair<String, String>> {
        val out = mutableListOf<Pair<String, String>>()
        nodes.forEach { (id, node) ->
            if (id == nodeId) return@forEach
            (node["inputs"] as? JsonObject)?.forEach { (name, v) ->
                if (refOf(v)?.first == nodeId) out += id to name
            }
        }
        return out
    }

    /** 从所有输出节点反向能走到的节点集合（= 出图用得上的骨干）。 */
    private fun liveNodes(nodes: Map<String, JsonObject>, outputs: List<String>): Set<String> {
        val live = LinkedHashSet<String>()
        outputs.forEach { id -> if (id in nodes) live += collect(id, nodes) }
        return live
    }

    /** 从某个输入出发，往回找第一个"没被摘掉"的上游 (节点id, 槽位)。 */
    private fun nearestLiveInput(
        nodeId: String,
        nodes: Map<String, JsonObject>,
        dropped: Set<String>,
    ): Pair<String, Int>? {
        val seen = mutableSetOf<String>()
        var cursor: String? = nodeId
        var guard = 0
        while (cursor != null && guard++ < 64) {
            if (!seen.add(cursor)) return null
            val node = nodes[cursor] ?: return null
            // 按声明顺序找第一个还在的连线上游
            (node["inputs"] as? JsonObject)?.forEach { (_, v) ->
                val ref = refOf(v) ?: return@forEach
                if (ref.first !in dropped && ref.first in nodes) return ref
            }
            // 这一层没有可用的：继续往回（它的上游本身也被摘了）
            cursor = (node["inputs"] as? JsonObject)?.values?.firstNotNullOfOrNull { refOf(it)?.first }
                ?.takeIf { it in nodes && it !in dropped }
        }
        return null
    }

    /** 一个输入算不算"接在某个节点上"（值是 `["id", slot]`）。 */
    private fun refOf(value: JsonElement?): Pair<String, Int>? {
        val arr = value as? JsonArray ?: return null
        if (arr.size < 2) return null
        val id = (arr[0] as? JsonPrimitive)?.contentOrNull ?: return null
        val slot = (arr[1] as? JsonPrimitive)?.contentOrNull?.toIntOrNull() ?: 0
        return id to slot
    }

    private fun splitPath(path: String): Pair<String, String>? {
        val dot = path.lastIndexOf('.')
        if (dot <= 0 || dot >= path.length - 1) return null
        return path.substring(0, dot) to path.substring(dot + 1)
    }

    /** 输出节点：类名带 Save / Preview / VideoCombine / Export。 */
    private fun isOutput(id: String, nodes: Map<String, JsonObject>): Boolean {
        val cls = nodes[id]?.cls()?.lowercase() ?: return false
        // 静音的输出节点不算（它自己不执行）
        val muted = (nodes[id]?.get("mode") as? JsonPrimitive)?.contentOrNull == "2"
        return !muted && OUTPUT_HINTS.any { cls.contains(it) }
    }

    /** [from] 出发顺着引用能走到的所有节点（含自己）。 */
    private fun collect(from: String, nodes: Map<String, JsonObject>): Set<String> {
        val out = mutableSetOf<String>()
        val stack = ArrayDeque(listOf(from))
        while (stack.isNotEmpty()) {
            val id = stack.removeLast()
            if (!out.add(id)) continue
            val node = nodes[id] ?: continue
            (node["inputs"] as? JsonObject)?.forEach { (_, v) ->
                refOf(v)?.first?.let { if (it in nodes) stack.addLast(it) }
            }
        }
        return out
    }

    /** 这个输出节点在图里还存不存在（存在就算"连得通"：空图才算断）。 */
    private fun reachable(target: String, nodes: Map<String, JsonObject>): Boolean =
        nodes.containsKey(target)

    /** 图里还有没有"指向不存在节点"的输入。 */
    private fun dangling(nodes: Map<String, JsonObject>): List<String> {
        val out = mutableListOf<String>()
        nodes.forEach { (id, node) ->
            (node["inputs"] as? JsonObject)?.forEach { (name, v) ->
                val ref = refOf(v) ?: return@forEach
                if (ref.first !in nodes) out += "$id.$name ← 节点 ${ref.first}（已不在图里）"
            }
        }
        return out
    }

    /**
     * 摘完之后**空着的连线型输入**（有价值的作业本：模型照着它补线）。
     *
     * 只看那些"本来就该有连线"的输入：没有 [objectInfo] 时退化成"值为空 / 长度不足的数组"。
     */
    private fun openWiredInputs(nodes: Map<String, JsonObject>, objectInfo: JsonObject?): List<String> {
        val out = mutableListOf<String>()
        nodes.forEach { (id, node) ->
            val cls = node.cls()
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            inputs.forEach { (name, v) ->
                val empty = v is JsonNull || (v is JsonPrimitive && v.isString && v.content.isBlank())
                if (!empty) return@forEach
                out += "$id.$name（$cls）"
            }
            if (objectInfo == null) return@forEach
            // 定义里声明为插口、但图里没有的输入也算缺口
            val spec = objectInfo[cls] as? JsonObject ?: return@forEach
            val required = ((spec["input"] as? JsonObject)?.get("required")) as? JsonObject ?: return@forEach
            required.forEach { (name, raw) ->
                if (name in inputs) return@forEach
                val decl = raw as? JsonArray ?: return@forEach
                val kind = (decl.firstOrNull() as? JsonPrimitive)?.contentOrNull ?: return@forEach
                if (!isWiredType(kind, decl)) return@forEach
                out += "$id.$name（$cls，缺 $kind）"
            }
        }
        return out
    }

    private fun isWiredType(type: String, decl: JsonArray): Boolean {
        val options = decl.getOrNull(1) as? JsonObject
        if ((options?.get("forceInput") as? JsonPrimitive)?.contentOrNull == "true") return true
        return type.uppercase() !in setOf("INT", "FLOAT", "STRING", "BOOLEAN", "COMBO")
    }

    /**
     * 补丁节点里"该接上游"的那个输入名。
     *
     * 优先找 `model`（Sage Attention / 各种 patch 节点都是这个约定），
     * 其次找定义里第一个连线型的必需输入。找不到就返回 null（宁可不插，也不乱接）。
     */
    private fun pickCarryInput(
        spec: JsonObject?,
        classType: String,
        already: Map<String, JsonElement>,
    ): String? {
        if ("model" !in already && (spec == null || spec["input"] != null)) {
            val required = ((spec?.get("input") as? JsonObject)?.get("required")) as? JsonObject
            if (required != null && "model" in required) return "model"
            if (spec == null && "model" !in already) return "model"
        }
        if (spec == null) return null
        val required = ((spec["input"] as? JsonObject)?.get("required")) as? JsonObject ?: return null
        required.forEach { (name, raw) ->
            if (name in already) return@forEach
            val decl = raw as? JsonArray ?: return@forEach
            val kind = (decl.firstOrNull() as? JsonPrimitive)?.contentOrNull ?: return@forEach
            if (isWiredType(kind, decl)) return name
        }
        return null
    }
}

/** 节点的类名（API 格式里是 `class_type`）。 */
private fun JsonObject.cls(): String = (this["class_type"] as? JsonPrimitive)?.contentOrNull.orEmpty()
