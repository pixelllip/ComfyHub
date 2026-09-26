package com.comfyhub

import com.comfyhub.ai.tools.ToolFailure
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.security.MessageDigest

/**
 * 本机 ComfyUI 里**已经保存的工作流文件**（`user\<用户>\workflows\*.json`）—— 用户 bug ⑤。
 *
 * 现场是一条真实对话：用户说"基于 krea2SFWNSFWUncensoredImageTo_v10 工作流生成"，
 * AI 回了"库里搜不到这条工作流，要么把 .json 路径发我，要么你在 ComfyUI 里点一次 Queue
 * 让它被自动捕获"。而那份文件**一直躺在磁盘上**：
 *
 * ```
 * D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2….json
 * ```
 *
 * 问题出在"库从哪里来"：`prompts` 表过去只有两个入口 —— 自动捕获（轮询 ComfyUI `/history`）
 * 与 `comfy_load_workflow`（用户/AI 主动给一个路径）。于是**用户机器上早已存在的工作流，
 * 在 AI 眼里等于不存在**：要么它在本项目进程活着的时候被跑过一次，要么用户手动把路径贴进对话。
 * 用户的原话："我需要在本项目进程存活的时候，产生新工作流运行的记录才会入库，这不对吧"。
 *
 * 这个文件负责"看得见"这件事，两条纪律：
 *
 *  1. **只读**：扫描与列出绝不改动用户的文件（[scan] 只读元数据 + 算 SHA-256）；
 *     入库（[importAll]）写的是**我们自己的库**，不是 ComfyUI 的目录。
 *  2. **有界 + 认特征**：只在 [ComfyRoots] 认出来的 ComfyUI 目录底下找
 *     `user\*\workflows`（一眼看完，不递归、不扫全盘），非 `.json`、空文件、
 *     超过 [MAX_FILE_BYTES] 的一律不列。
 *
 * 目录怎么找（三份来源，与读白名单同一套判据，绝不"看到个叫 workflows 的目录就认"）：
 *  - 配置里已生效的输出目录的**父目录**（`<ComfyUI>\output` 的父目录就是 ComfyUI 根）；
 *  - [ComfyLocator] 探测到的 home；
 *  - [ComfyRoots.autoReadRoots] —— ComfyUI Desktop 把程序与共享数据分家时，
 *    工作流目录在**安装目录**下（`…\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows`），
 *    只按配置的输出目录推是推不到的（用户 bug ④ 就是这个问题）。
 */
object ComfyWorkflowFiles {

    private val log = LoggerFactory.getLogger(ComfyWorkflowFiles::class.java)

    /** 一份工作流文件最大认多大（与 `AiWorkflowSearch.MAX_WORKFLOW_FILE_BYTES` 一致）。 */
    const val MAX_FILE_BYTES = 32L * 1024 * 1024

    /** 一次最多列几份（用户的工作流目录通常几十份；上万份一定是认错目录了）。 */
    const val MAX_FILES = 200

    /** 一个 ComfyUI 根下面最多看几个用户目录（`user\` 里正常只有 default / __manager）。 */
    private const val MAX_USERS = 20

    /**
     * 本机工作流目录里的一份文件。
     *
     * [sha256] 是**内容寻址**的键：`comfy_load_workflow` 入库时用的 `run_key = file:<sha256>`，
     * 所以拿它就能问"这份文件是不是已经在库里了"（[promptId] 非空就是）。
     */
    @Serializable
    data class WorkflowFile(
        val name: String,
        val path: String,
        val bytes: Long = 0,
        val modifiedAt: String? = null,
        /** `ui` = 界面格式（nodes/links）；`api` = 「导出（API）」的节点图；`unknown` = 读不出来 */
        val format: String = "unknown",
        val sha256: String = "",
        /** 已经在本项目库里的话，这是那份提示词的 id（可以直接 `comfy_submit(promptId=…)`） */
        val promptId: Long? = null,
    )

    /** 入库结果里的一份文件（[status] = imported / duplicate / failed）。 */
    @Serializable
    data class ImportItem(
        val name: String,
        val path: String,
        val status: String,
        val promptId: Long? = null,
        /** 库里的那份现在能不能直接跑（转换缺口见 [reason]） */
        val runnable: Boolean? = null,
        val reason: String? = null,
    )

    // -----------------------------------------------------------------------
    //  目录发现
    // -----------------------------------------------------------------------

    /** 本机所有**实际存在**的 `workflows` 目录（去重、保持"最可能命中"的顺序）。 */
    fun dirs(cfg: AppConfig): List<Path> = dirsFromHomes(comfyHomes(cfg))

    /**
     * 一组 ComfyUI 根目录 → 它们底下实际存在的 `user\<用户>\workflows`。
     *
     * 与 [comfyHomes] 分开是为了能单测：这一半是纯文件系统逻辑（临时目录就能验），
     * 那一半才需要配置与数据库。
     */
    fun dirsFromHomes(homes: Collection<Path>): List<Path> {
        val out = LinkedHashSet<Path>()
        homes.forEach { userWorkflowDirs(it, out) }
        return out.toList()
    }

    /** 本机可能装着 ComfyUI 的那些目录（三份来源，都可能为空）。 */
    private fun comfyHomes(cfg: AppConfig): List<Path> {
        val homes = LinkedHashSet<Path>()

        // 1) 配置里已经生效的输出目录：`<ComfyUI>\output` 的父目录就是 ComfyUI 的根。
        //    读库失败（首次启动还没建库）不该让后面两条更可靠的来源一起丢掉。
        runCatching { SettingsRepo.captureConfig(cfg).outputDir }.getOrNull()
            ?.takeIf { it.isNotBlank() }
            ?.let { raw ->
                realDir(runCatching { Paths.get(raw) }.getOrNull())?.let { dir ->
                    val home = if (dir.fileName?.toString().equals("output", ignoreCase = true)) dir.parent else dir
                    realDir(home)?.let(homes::add)
                }
            }

        // 2) 探测到的 home（只读，找不到就是没有）
        runCatching { ComfyLocator.find(cfg).home }.getOrNull()
            ?.takeIf { it.isNotBlank() }
            ?.let { raw -> realDir(runCatching { Paths.get(raw) }.getOrNull())?.let(homes::add) }

        // 3) 自动放行的那些 ComfyUI 目录（含 Desktop 的安装目录与它的兄弟目录，见 ComfyRoots）
        runCatching { ComfyRoots.autoReadRoots(cfg.projectRoot) }.getOrNull()?.forEach(homes::add)

        return homes.toList()
    }

    /**
     * 一个 ComfyUI 根底下的 `user\<用户>\workflows`。
     *
     * 看的是 `user\` 的**直接子目录**（正常只有 `default`，多个用户时每个一份），
     * 不递归、不跟随符号链接 —— 这一层目录很浅，而 `models\` 那种几百 GB 的目录根本不在路径上。
     */
    private fun userWorkflowDirs(home: Path, out: MutableSet<Path>) {
        val user = home.resolve("user")
        if (!Files.isDirectory(user)) return
        val users = runCatching { Files.newDirectoryStream(user).use { it.toList() } }.getOrNull() ?: return
        for (u in users.sortedBy { it.fileName.toString().lowercase() }.take(MAX_USERS)) {
            if (Files.isSymbolicLink(u)) continue
            if (!runCatching { Files.isDirectory(u) }.getOrDefault(false)) continue
            val wf = u.resolve("workflows")
            if (Files.isDirectory(wf)) {
                (runCatching { wf.toRealPath() }.getOrNull() ?: wf).let(out::add)
            }
        }
    }

    private fun realDir(p: Path?): Path? {
        if (p == null) return null
        return runCatching {
            val abs = p.toAbsolutePath().normalize()
            if (!Files.isDirectory(abs)) null else abs.toRealPath()
        }.getOrNull()
    }

    // -----------------------------------------------------------------------
    //  列文件（只读）
    // -----------------------------------------------------------------------

    /**
     * 列出这些目录里的工作流文件（按修改时间倒序，最多 [limit] 份）。
     *
     * [query] 是**文件名**的子串匹配（大小写不敏感）—— 用户说"krea2 那条"，AI 就能直接命中。
     * 只列**真的能用**的：非 `.json` / 空文件 / 坏 JSON / 既不是界面格式也不是 API 格式的
     * 全部跳过（列出来只会让 AI 白跑一趟 `comfy_load_workflow`）。
     * 这里不算库里的对应关系（那是 [listJson] 与库对账时做的事），所以它是纯磁盘操作，能单测。
     */
    fun scan(dirs: List<Path>, query: String? = null, limit: Int = 40): List<WorkflowFile> {
        val q = query?.trim()?.lowercase()?.takeIf { it.isNotEmpty() }
        val cap = limit.coerceIn(1, MAX_FILES)
        val out = mutableListOf<WorkflowFile>()
        for (dir in dirs) {
            val children = runCatching { Files.newDirectoryStream(dir).use { it.toList() } }.getOrNull() ?: continue
            for (f in children) {
                val name = f.fileName?.toString() ?: continue
                if (name.startsWith(".")) continue
                if (!name.lowercase().endsWith(".json")) continue
                if (q != null && !name.lowercase().contains(q)) continue
                if (!runCatching { Files.isRegularFile(f) }.getOrDefault(false)) continue
                val size = runCatching { Files.size(f) }.getOrDefault(0L)
                if (size <= 0L || size > MAX_FILE_BYTES) continue
                val file = describe(f, name, size) ?: continue
                // 解析不出来、或者既不是界面格式也不是 API 格式的一律不列：
                // 它们 `comfy_load_workflow` 一定失败（NOT_A_WORKFLOW），列出来只会让 AI 白跑一趟
                if (file.format == "unknown") continue
                out += file
            }
        }
        // ISO-8601 字符串的字典序就是时间序（同一时区、同一格式），不必再解析
        return out.sortedByDescending { it.modifiedAt ?: "" }.take(cap)
    }

    private fun describe(file: Path, name: String, size: Long): WorkflowFile? {
        val bytes = runCatching { Files.readAllBytes(file) }.getOrNull() ?: return null
        val modified = runCatching { Files.getLastModifiedTime(file).toInstant() }.getOrNull()
        return WorkflowFile(
            name = name,
            path = file.toString(),
            bytes = size,
            modifiedAt = modified?.toString(),
            format = formatOf(bytes),
            sha256 = sha256Hex(bytes),
        )
    }

    /**
     * 这份文件是界面格式、API 格式，还是读不出来。
     *
     * 与 `AiWorkflowSearch.loadWorkflowFromFile` 的判据**完全同一份**
     * （[WorkflowConvert.isUiWorkflow] / [HistoryEntry.isNodeGraph]）——
     * 两边不一致就会出现"列表说能跑、加载说不是工作流"这种最难查的分歧。
     */
    private fun formatOf(bytes: ByteArray): String {
        val root = runCatching {
            AppJson.parseToJsonElement(String(bytes, Charsets.UTF_8)) as? JsonObject
        }.getOrNull() ?: return "unknown"
        return when {
            HistoryEntry.isNodeGraph(root) -> "api"
            WorkflowConvert.isUiWorkflow(root) -> "ui"
            else -> "unknown"
        }
    }

    /**
     * 给模型 / 界面看的清单 JSON。
     *
     * [libraryPromptId] 由调用方注入（拿 sha256 问库）—— 这样这个文件不直接依赖 `CaptureRepo`，
     * 磁盘扫描那部分可以在没有数据库的情况下单测。
     */
    fun listJson(
        dirs: List<Path>,
        query: String?,
        limit: Int,
        libraryPromptId: (String) -> Long? = { null },
    ): JsonObject {
        val files = scan(dirs, query, MAX_FILES).map { it.copy(promptId = libraryPromptId(it.sha256)) }
        val shown = files.take(limit.coerceIn(1, MAX_FILES))
        return buildJsonObject {
            put("count", shown.size)
            put("total", files.size)
            put("dirs", buildJsonArray { dirs.forEach { add(JsonPrimitive(it.toString())) } })
            put(
                "files",
                buildJsonArray {
                    shown.forEach { f ->
                        add(
                            buildJsonObject {
                                put("name", f.name)
                                put("path", f.path)
                                put("bytes", f.bytes)
                                put("modifiedAt", f.modifiedAt)
                                put("format", f.format)
                                put("inLibrary", f.promptId != null)
                                f.promptId?.let { put("promptId", it) }
                            }
                        )
                    }
                },
            )
            put(
                "hint",
                "这些工作流**不需要先跑一次**：拿到 path 直接 comfy_load_workflow(path=…) 读进库，" +
                    "再用返回的 promptId 提交。inLibrary=true 的那几份库里已经有了（promptId 就是它）。",
            )
            if (dirs.isEmpty()) {
                put(
                    "message",
                    "没找到本机 ComfyUI 的 workflows 目录（`user\\<用户>\\workflows`）。" +
                        "可能是没装 ComfyUI，或者它的位置没被认出来 —— 让用户在「设置 → ComfyUI 自动捕获」里" +
                        "确认 ComfyUI 位置，或者直接把工作流文件的完整路径给过来。",
                )
            } else if (shown.isEmpty()) {
                put(
                    "message",
                    if (query.isNullOrBlank()) "这些目录里一个 .json 工作流都没有：$dirs"
                    else "本机工作流里没有文件名匹配「$query」的（共 ${files.size} 份，可以不带 query 再列一次）",
                )
            }
        }
    }

    // -----------------------------------------------------------------------
    //  批量入库（写的是我们自己的库，不动用户的文件）
    // -----------------------------------------------------------------------

    /**
     * 把本机工作流目录里的文件**批量读进库**（界面上的「导入工作流文件…」与
     * `POST /api/capture/import-workflows` 走的就是它）。
     *
     * 这是 [ComfyWorkflowFiles] 存在的第二个理由：AI 首轮就能列到这些文件，
     * 但用户想在自己的「提示词」库里也看到它们 —— 那时不用等 AI 一条条读。
     *
     * 三条与单条加载一致的纪律：
     *  - **幂等**：`run_key = file:<sha256>`，同一份文件重复导入只会得到同一个 promptId（[ImportItem.status] = `duplicate`）；
     *  - **不因"转换不了"整批失败**：带缺口的前端节点用 `tolerateUnsupported=true` 摘掉，
     *    那份仍然入库并标 `runnable=false`，缺口由 AI 照着 `comfy_load_workflow` 的清单补线；
     *  - **如实报错**：单个文件失败不影响别的，原因逐条列在 [ImportItem.reason] 里
     *    （ComfyUI 没起来时界面格式转不了 → 每一条都会写 `COMFY_UNREACHABLE`，不假装成功）。
     */
    suspend fun importAll(
        cfg: AppConfig,
        submitter: ComfySubmitter,
        dirs: List<Path>,
        limit: Int = MAX_FILES,
        query: String? = null,
    ): JsonObject {
        val files = scan(dirs, query, limit.coerceIn(1, MAX_FILES))
        var imported = 0
        var duplicates = 0
        var failed = 0
        val ids = mutableListOf<Long>()
        val items = mutableListOf<ImportItem>()

        for (f in files) {
            val runKey = "file:${f.sha256}"
            val existing = runCatching { CaptureRepo.findRun(runKey) }.getOrNull()
            if (existing?.promptId != null) {
                duplicates++
                items += ImportItem(f.name, f.path, "duplicate", existing.promptId)
                continue
            }
            try {
                val loaded = AiWorkflowSearch.loadWorkflowFromFile(
                    submitter = submitter,
                    file = Paths.get(f.path),
                    title = null,
                    includeGraph = false,
                    // 带缺口也入库：批量导入时"转换不了"绝不能让整份工作流消失，
                    // 它照样是用户库里的一份资产（能不能跑由 runnable 如实标出来）
                    tolerateUnsupported = true,
                )
                val id = (loaded["promptId"] as? JsonPrimitive)?.contentOrNull?.toLongOrNull()
                val runnable = (loaded["runnable"] as? JsonPrimitive)?.contentOrNull?.toBoolean()
                imported++
                id?.let { ids += it }
                items += ImportItem(
                    name = f.name,
                    path = f.path,
                    status = "imported",
                    promptId = id,
                    runnable = runnable,
                    reason = if (runnable == false) {
                        "转换时摘掉了纯前端节点，缺口见 comfy_load_workflow 的 openInputs / unresolvedInputs"
                    } else null,
                )
            } catch (e: ToolFailure) {
                failed++
                items += ImportItem(f.name, f.path, "failed", reason = "${e.code}: ${e.message?.take(200)}")
            } catch (e: Exception) {
                failed++
                items += ImportItem(f.name, f.path, "failed", reason = "${e.javaClass.simpleName}: ${e.message?.take(200)}")
            }
        }

        log.info(
            "导入本机工作流：目录={} 扫到 {} 份，新导入 {}，已在库 {}，失败 {}",
            dirs, files.size, imported, duplicates, failed,
        )
        return buildJsonObject {
            put("dirs", buildJsonArray { dirs.forEach { add(JsonPrimitive(it.toString())) } })
            put("scanned", files.size)
            put("imported", imported)
            put("duplicates", duplicates)
            put("failed", failed)
            put("promptIds", buildJsonArray { ids.forEach { add(JsonPrimitive(it)) } })
            put(
                "items",
                buildJsonArray {
                    items.forEach { i ->
                        add(
                            buildJsonObject {
                                put("name", i.name)
                                put("path", i.path)
                                put("status", i.status)
                                i.promptId?.let { put("promptId", it) }
                                i.runnable?.let { put("runnable", it) }
                                i.reason?.let { put("reason", it) }
                            },
                        )
                    }
                },
            )
            put(
                "message",
                when {
                    dirs.isEmpty() ->
                        "没找到本机 ComfyUI 的 workflows 目录（`user\\<用户>\\workflows`）—— " +
                            "先在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 的位置。"
                    files.isEmpty() ->
                        "这些目录里没有工作流文件（.json）：${dirs.joinToString("、")}"
                    else ->
                        "扫描 ${files.size} 份文件：新导入 $imported，已在库里 $duplicates，失败 $failed"
                },
            )
        }
    }

    private fun sha256Hex(bytes: ByteArray): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(bytes)
        return digest.joinToString("") { "%02x".format(it) }
    }
}
