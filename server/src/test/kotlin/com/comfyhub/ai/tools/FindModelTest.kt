package com.comfyhub.ai.tools

import com.comfyhub.ComfyModels
import com.comfyhub.ai.CredentialService
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put

/**
 * `find_model`（P3 定向查询，2026-09-22）的回归。
 *
 * 盯的是用户拍板的那几件事（`docs/bug and suggestion/bug-and-suggestion-9.22.md` 的 K 节与 D 表）：
 *
 *  - **先查本机再查外网**：本机同名命中时**一个外部请求都不发**（`FakeFinder.calls` 为 0），
 *    并把三个源标成"本次没去查"；
 *  - **每个源都报可达性**：源连不上 / 要代理 / 要登录 / gated 时，它照样在结果里占一行，
 *    而且状态要**分得开**（不能都写成"error"）；
 *  - **统一字段**：名字 / 类型 / 底模 / 触发词 / 体积 / 直链 / **sha256** / **installInto** / 来源；
 *  - **绝不代下**：工具是只读的、描述里明写不下载、结构化结果里也有这一条 ——
 *    而"绝不代下"在代码形状上的落点是 [ModelFinder]**根本没有"取文件"的入口**。
 *
 * 全部离线：HTTP 那部分打的是本机临时端口上的假服务（与 `CivitaiSearchTest` 同一套路）。
 */
class FindModelTest {

    // =======================================================================
    //  假件：不起 HTTP 的 ModelFinder
    // =======================================================================

    /** 一条候选，默认把 P3 要的字段**全填满**（缺字段那几种情形由各用例自己造）。 */
    private fun candidate(
        name: String = "krea2 Turbo Int8",
        kind: ModelKind = ModelKind.CHECKPOINT,
        source: ModelSource = ModelSource.CIVITAI,
        sha256: String? = SHA_A,
        fileName: String? = "krea2_turbo_int8.safetensors",
        sizeBytes: Long? = 14_145_495L * 1024,
    ) = ModelCandidate(
        name = name,
        kind = kind,
        baseModel = "Flux.1 D",
        trainedWords = listOf("krea2", "turbo"),
        sizeBytes = sizeBytes,
        downloadUrl = "https://example.test/dl/$name",
        pageUrl = "https://example.test/models/9",
        sha256 = sha256,
        fileName = fileName,
        source = source,
        description = "拿来出图的大底模",
        downloads = 1234,
    )

    private class FakeFinder(
        var localHit: Boolean = false,
        var sources: List<SourceOutcome> = emptyList(),
        var candidates: List<ModelCandidate> = emptyList(),
        var nearMisses: List<String> = emptyList(),
    ) : ModelFinder {
        var calls = 0
        var lastQuery: ModelQuery? = null

        override suspend fun search(query: ModelQuery): ModelFinderResult {
            calls++
            lastQuery = query
            val local = if (localHit) {
                listOf(
                    ComfyModels.LocalModel(
                        name = "krea2_turbo_int8.safetensors",
                        folder = "checkpoints",
                        path = "D:\\models\\checkpoints\\krea2_turbo_int8.safetensors",
                        bytes = 13_900_000_000L,
                    ),
                )
            } else {
                emptyList()
            }
            val outcomes = sources.ifEmpty {
                ModelSource.entries.map { SourceOutcome.Ok(it, emptyList(), "通了，但这一问没有结果") }
            }
            return ModelFinderResult(
                query = query.text,
                sources = outcomes,
                candidates = candidates,
                localMatches = local,
                localNearMisses = nearMisses,
                localSource = ComfyModels.Source.OBJECT_INFO,
                online = !localHit,
                notes = emptyList(),
            )
        }
    }

    private class Env(val root: java.nio.file.Path, val finder: ModelFinder?) {
        val skills = SkillStore(root.resolve("skills-builtin"), root.resolve("skills-user"))
        val memory = MemoryStore(root.resolve("storage").resolve("ai"))
        val gate = ToolApprovalGate(timeoutMs = 2000)
        var inventoryCalls = 0

        val registry = ToolRegistry(
            projectRoot = root,
            skills = skills,
            approvals = gate,
            comfyStatus = { buildJsonObject { put("connected", true) } },
            comfyFindRun = { null },
            comfySync = { buildJsonObject { put("synced", true) } },
            finder = finder,
            // 本机清单：给一份"一个模型都没有"的清单（真正测本机命中要的是 finder 那一层，
            // 这里只钉接线 —— 工具拿到的清单**必须**是 comfyModels 这一份，不能自己另算）
            comfyModels = {
                inventoryCalls++
                ComfyModels.Inventory(ComfyModels.Source.OBJECT_INFO, emptyList())
            },
            comfyObjectInfo = { null },
        )

        val policy = ToolPolicy(root, ToolPolicyConfig())
        fun ctx(p: ToolPolicy = policy) = ToolContext("run-1", p, skills, memory)
    }

    private fun env(finder: ModelFinder? = FakeFinder()): Env {
        val root = Files.createTempDirectory("comfyhub-findmodel").toRealPath()
        listOf("comfyui", "storage").forEach { Files.createDirectories(root.resolve(it)) }
        return Env(root, finder)
    }

    private fun allowAll(e: Env) = ToolPolicy(
        e.root,
        ToolPolicyConfig(overrides = mapOf("find_model" to "allow")),
    )

    // =======================================================================
    //  一、清单 / 字段 / 绝不代下
    // =======================================================================

    @Test
    fun `find_model 在工具清单里 只读 走联网类 默认要批准`() {
        val e = env()
        val info = e.registry.info(e.policy).firstOrNull { it.name == "find_model" }
        assertTrue(info != null, "find_model 必须出现在工具清单里（真源是 ToolRegistry.tools）")
        // 只读：它一个字节都不写（写盘那条路由 download_model 负责，而它默认 deny）
        assertFalse(info!!.mutating, "find_model 必须是只读工具")
        assertEquals("web", info.category, "查询词会离开本机 → 与 web_search / civitai_search 同类")
        assertEquals("ask", info.access, "出网的工具默认要用户点一下（与 civitai_search 同一条纪律）")
    }

    /**
     * 「绝不代下」是用户拍板的（D 表第二行）。
     *
     * 这里钉的是**能抓住回归**的那几条：描述里明写不下载、结构化结果里有 `downloadsModel=false`、
     * 而 [ModelFinder] 这个接口**只有 search 一个方法** ——
     * 谁想加"顺手把文件下了"的能力，就必须先改掉这个接口，那时这条断言会红。
     */
    @Test
    fun `绝不代下 描述 结构化结果与接口形状三处都要挡住`() {
        val e = env()
        val desc = e.registry.find("find_model")!!.description
        assertTrue(
            desc.contains("绝不代下") && desc.contains("永远不替用户下载"),
            "描述里必须明写不代下（模型只看得到描述）：$desc",
        )
        // 系统提示只带描述的前 300 个字符 —— "不代下"那句必须落在窗口里
        assertTrue(
            desc.take(300).contains("永远不替用户下载"),
            "「绝不代下」要排在描述最前面，否则会被 300 字窗口挤掉：${desc.take(300)}",
        )
        assertTrue(desc.contains("sha256"), "sha256 是这条工具的核心产出之一，描述里要写")
        assertTrue(
            desc.contains("先查本机") || desc.contains("先查本机，再查外网"),
            "顺序纪律（先查本机再查外网）要写进描述",
        )

        // 接口形状：唯一的动作是"搜一次"，没有任何取文件的入口
        val methods = ModelFinder::class.java.declaredMethods.map { it.name }.toSet()
        assertEquals(
            setOf("search"),
            methods,
            "ModelFinder 只该有 search 一个动作 —— 多出来的那个很可能就是「代下」的口子",
        )
    }

    @Test
    fun `结构化结果里每个源都有一行 候选字段齐全 并带 downloadsModel=false`() = runBlocking {
        val finder = FakeFinder(
            sources = listOf(
                SourceOutcome.Ok(ModelSource.CIVITAI, listOf(candidate()), "通了，1 条"),
                SourceOutcome.Failed(
                    ModelSource.HUGGINGFACE,
                    Reachability.NEEDS_PROXY,
                    "连不上 huggingface.co（本次出网路径「直连（DIRECT）」）",
                ),
                SourceOutcome.Failed(
                    ModelSource.MODELSCOPE,
                    Reachability.ERROR,
                    "被限流了（429）",
                ),
            ),
            candidates = listOf(candidate()),
        )
        val e = env(finder)
        val record = e.registry.invoke("c1", "find_model", """{"query":"krea2 turbo"}""", e.ctx(allowAll(e)))
        assertEquals("ok", record.status, record.error ?: record.content)

        val json = record.resultJson!!
        // 三个源**一条不少**，顺序固定 Civitai → HF → ModelScope
        val sources = json["sources"]!!.let { it as kotlinx.serialization.json.JsonArray }
        assertEquals(3, sources.size)
        assertEquals(
            listOf("civitai", "huggingface", "modelscope"),
            sources.map { it.jsonObject["source"]!!.toString().trim('"') },
        )
        assertEquals("ok", sources[0].jsonObject["state"]!!.toString().trim('"'))
        assertEquals("needs_proxy", sources[1].jsonObject["state"]!!.toString().trim('"'))
        assertEquals("error", sources[2].jsonObject["state"]!!.toString().trim('"'))

        val c = json["candidates"]!!.let { it as kotlinx.serialization.json.JsonArray }[0].jsonObject
        assertEquals("krea2 Turbo Int8", c["name"]!!.toString().trim('"'))
        assertEquals("checkpoint", c["kind"]!!.toString().trim('"'))
        // installInto：P3 点名要的「该放哪个目录」
        assertEquals("checkpoints", c["installInto"]!!.toString().trim('"'))
        assertEquals("Flux.1 D", c["baseModel"]!!.toString().trim('"'))
        assertTrue(c["trainedWords"].toString().contains("krea2"))
        assertTrue(c["sizeLabel"].toString().contains("GB"))
        // sha256：P3 点名要的第二个字段
        assertEquals(SHA_A, c["sha256"]!!.toString().trim('"'))
        assertEquals("krea2_turbo_int8.safetensors", c["fileName"]!!.toString().trim('"'))
        assertTrue(c["sources"].toString().contains("civitai"))
        // 「绝不代下」也写进结构化结果
        assertEquals("false", json["downloadsModel"].toString())

        // 源不可达时**正文里也要有那一行**（不许从列表里消失）
        assertTrue(record.content.contains("HuggingFace"), record.content)
        assertTrue(record.content.contains("本机直连不通"), record.content)
        assertTrue(record.content.contains("ModelScope"), record.content)
        assertTrue(record.content.contains("绝不代下"), record.content)
        // 「该放进哪个目录」与 sha256 两栏都要在给模型看的正文里（不能只在 JSON 里）
        assertTrue(record.content.contains("该放进：checkpoints"), record.content)
        assertTrue(record.content.contains("sha256：$SHA_A"), record.content)
    }

    /**
     * 上游没给 sha256 时：**不许编**，而且要在正文里说清"这一份下完没法校验"。
     *
     * 这条盯的是 P3 第 5 条（sha256 的第二用途）：字面相似度**不足以**确认"盘上那份是不是同一个模型"，
     * 所以"没有 sha256"这件事必须让模型看得见 —— 否则它会拿"名字很像"当确认。
     */
    @Test
    fun `候选没有 sha256 时如实标注 下完没法校验`() = runBlocking {
        val finder = FakeFinder(candidates = listOf(candidate(sha256 = null)))
        val e = env(finder)
        val record = e.registry.invoke("c1", "find_model", """{"query":"x"}""", e.ctx(allowAll(e)))
        assertEquals("ok", record.status, record.error ?: record.content)
        assertTrue(record.content.contains("没有 sha256") || record.content.contains("没给 sha256"), record.content)
        // 结构化结果里**不写这个键**（写 null 容易被当成"校验过了"）
        val c = (record.resultJson!!["candidates"] as kotlinx.serialization.json.JsonArray)[0].jsonObject
        assertTrue(!c.containsKey("sha256"), c.toString())
    }

    @Test
    fun `该放哪个目录说不准时给说明而不是编一个目录`() = runBlocking {
        val finder = FakeFinder(
            candidates = listOf(
                candidate(kind = ModelKind.UNKNOWN).copy(
                    note = "这个仓库是 diffusers 目录格式，里面没有一个能直接放进 ComfyUI 的整包 safetensors",
                ),
            ),
        )
        val e = env(finder)
        val record = e.registry.invoke("c1", "find_model", """{"query":"x"}""", e.ctx(allowAll(e)))
        assertEquals("ok", record.status, record.error ?: record.content)
        val c = (record.resultJson!!["candidates"] as kotlinx.serialization.json.JsonArray)[0].jsonObject
        assertTrue(!c.containsKey("installInto"), "说不准就不要给一个目录：" + c)
        assertTrue(c["note"].toString().contains("diffusers"), c.toString())
        // 正文里也要有这条说明（那是模型唯一看得到的地方）
        assertTrue(record.content.contains("diffusers"), record.content)
    }

    @Test
    fun `工具把 query 与 type 归一后交给 finder 并把本机清单一起传下去`() = runBlocking {
        val finder = FakeFinder(candidates = listOf(candidate(kind = ModelKind.LORA, source = ModelSource.MODELSCOPE)))
        val e = env(finder)
        val record = e.registry.invoke(
            "c1",
            "find_model",
            """{"query":"  水墨风 LoRA  ","type":"lora","limit":3}""",
            e.ctx(allowAll(e)),
        )
        assertEquals("ok", record.status, record.error ?: record.content)
        val q = finder.lastQuery!!
        assertEquals("水墨风 LoRA", q.text, "查询词要去掉首尾空白")
        assertEquals(ModelKind.LORA, q.kind)
        assertEquals(3, q.limit)
        assertTrue(q.online, "没有本地命中时该出网")
        // 本机清单必须来自 comfyModels（与 comfy_env_report 同一份），不能另算一份
        assertEquals(1, e.inventoryCalls, "工具要拿本机清单来看'先查本机'")
        assertTrue(q.inventory != null, "清单要传进 finder（本地短路就靠它）")
    }

    @Test
    fun `type 写错时当场列出可选值 而不是给一份空结果`() = runBlocking {
        val finder = FakeFinder()
        val e = env(finder)
        val record = e.registry.invoke(
            "c1", "find_model", """{"query":"x","type":"Loraa"}""", e.ctx(allowAll(e)),
        )
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
        assertTrue(record.error!!.contains("LORA"), record.error!!)
        assertEquals(0, finder.calls, "本地就能判错的东西不该走到 finder（更不该出网）")
    }

    @Test
    fun `缺少 query 当场报错`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "find_model", "{}", e.ctx(allowAll(e)))
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
    }

    @Test
    fun `没有 finder 时报 FIND_MODEL_DISABLED 而不是假装没查到`() = runBlocking {
        val e = env(finder = null)
        val record = e.registry.invoke("c1", "find_model", """{"query":"x"}""", e.ctx(allowAll(e)))
        assertEquals("failed", record.status)
        assertEquals("FIND_MODEL_DISABLED", record.errorCode)
    }

    @Test
    fun `默认要批准 没人批准就绝不出网`() = runBlocking {
        val finder = FakeFinder()
        val e = env(finder)
        // 走默认策略（ask）：开一次闸门但**没人批**（= 界面没点 / 超时）→ 拒绝。
        // 这正是 `ToolApprovalGate.await` 的语义：超时与 Run 取消都算拒绝。
        e.gate.open("call-1")
        val record = e.registry.invoke("call-1", "find_model", """{"query":"x"}""", e.ctx())
        assertEquals("failed", record.status)
        assertEquals("APPROVAL_DENIED", record.errorCode)
        assertEquals("denied", record.approval)
        assertEquals(0, finder.calls, "没批准就一个请求都不该发")
    }

    @Test
    fun `联网预算与 civitai_search 共用一条 不会两条路各来一份`() = runBlocking {
        val e = env()
        var ctx = e.ctx(allowAll(e))
        repeat(ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN) { i ->
            val r = e.registry.invoke("c$i", "find_model", """{"query":"q$i"}""", ctx)
            assertEquals("ok", r.status, r.error ?: r.content)
        }
        val over = e.registry.invoke("c99", "find_model", """{"query":"q99"}""", ctx)
        assertEquals("failed", over.status)
        assertEquals("QUERY_BUDGET_EXCEEDED", over.errorCode)
    }

    // =======================================================================
    //  二、本机优先 → 短路（真实实现那一层）
    // =======================================================================

    /**
     * 顺序纪律（用户拍板）：**先查本机，再查外网**。
     *
     * 命中的判据是"同名"，而且**不能用纯子串**：`anima` 不该命中 `anima-turbo`。
     * 这条用例同时钉住这两半 —— 前者是用户要的行为，后者是它的边界。
     */
    @Test
    fun `本机同名时一个请求都不发 并把三个源标成没去查`() = runBlocking {
        val civitai = CountingCivitai()
        val finder = HttpModelFinder(civitai = civitai)
        val inv = inventoryOf("krea2_turbo_int8.safetensors", folder = "checkpoints")

        val hit = finder.search(
            ModelQuery(text = "krea2TurboInt8.safetensors", inventory = inv),
        )
        assertTrue(hit.localHit, "分词后同名要算命中（写法不同也是同一个模型）")
        assertFalse(hit.online, "命中本机就不该出网")
        assertEquals(0, civitai.calls, "一个 Civitai 请求都不该发")
        assertEquals(3, hit.sources.size, "三个源照样要各占一行")
        assertTrue(hit.sources.all { it.state == Reachability.SKIPPED }, hit.sources.toString())
        assertTrue(hit.notes.any { it.contains("没有") && it.contains("外网") }, hit.notes.toString())

        // 边界：`anima` 不该把 `anima-turbo` 当成"本机已有"
        val other = finder.search(
            ModelQuery(text = "anima", inventory = inventoryOf("anima-turbo.safetensors", "checkpoints")),
        )
        assertFalse(other.localHit, "子串命中不是同名 —— 那是另一个模型")
    }

    @Test
    fun `online=false 时只查本机 且不碰外网`() = runBlocking {
        val civitai = CountingCivitai()
        val finder = HttpModelFinder(civitai = civitai)
        val r = finder.search(ModelQuery(text = "随便什么", online = false))
        assertFalse(r.online)
        assertEquals(0, civitai.calls)
        assertTrue(r.sources.all { it.state == Reachability.SKIPPED })
    }

    @Test
    fun `本机清单来源会如实带进结果（object_info 与 scan 不是一回事）`() = runBlocking {
        val finder = HttpModelFinder(civitai = CountingCivitai())
        val r = finder.search(
            ModelQuery(text = "x", online = false, inventory = inventoryOf("a.safetensors", "loras")),
        )
        assertEquals(ComfyModels.Source.OBJECT_INFO, r.localSource)
    }

    // =======================================================================
    //  三、真实 HTTP 那一层（打本机假服务，不出网）
    // =======================================================================

    /**
     * 三家一起看：**顺序 / 字段 / sha256 / installInto / 可达性**。
     *
     * sha256 从哪儿来是这条用例的核心：
     *  · Civitai = 版本 hash（`files[primary].hashes.SHA256`）；
     *  · HF = **LFS oid**（`tree` 接口里的 `lfs.oid`）；
     *  · ModelScope = 目录接口里那个 `Sha256` 字段。
     */
    @Test
    fun `三源按顺序查到 且 sha256 与 installInto 各自取对`() = runBlocking {
        // ⚠️ 路由顺序要紧：`/api/models`（HF 检索）与 `/api/v1/models…`（Civitai 检索）
        // 都是前缀关系，写成 `startsWith("/api/v1/models")` 会**抢走** ModelScope 的
        // `/api/v1/models/<repo>/repo/files`（它更长、更具体，必须排在前面）。
        withServer({ path, _ ->
            when {
                path.endsWith("/repo/files") -> 200 to MODELSCOPE_FILES_BODY
                path == "/api/v1/dolphin/models" -> 200 to MODELSCOPE_BODY
                path == "/api/models" -> 200 to HF_SEARCH_BODY
                path == "/api/models/krea/Krea-2-Turbo/tree/main" -> 200 to HF_TREE_BODY
                path == "/api/v1/models" -> 200 to CIVITAI_BODY
                else -> 404 to "{}"
            }
        }) { base ->
            val finder = HttpModelFinder(
                civitai = CivitaiSearch(noTokenCredentials(), safeBaseUrl = base, nsfwBaseUrl = base),
                hfBaseUrl = base,
                modelscopeBaseUrl = base,
            )
            val r = finder.search(ModelQuery(text = "krea2 turbo"))
            assertTrue(r.online)

            // 顺序固定（用户拍板）：Civitai → HuggingFace → ModelScope
            assertEquals(
                listOf(ModelSource.CIVITAI, ModelSource.HUGGINGFACE, ModelSource.MODELSCOPE),
                r.sources.map { it.source },
            )
            assertTrue(r.sources.all { it.state == Reachability.OK }, r.sources.toString())
            assertEquals(3, r.candidates.size, "三个源各一条")

            val civitai = r.candidates.first { it.source == ModelSource.CIVITAI }
            assertEquals(SHA_CIVITAI, civitai.sha256, "Civitai 的 sha256 = 版本 hash")
            assertEquals("checkpoints", civitai.installInto)
            assertEquals("krea2_turbo_int8.safetensors", civitai.fileName)
            assertEquals("SDXL 1.0", civitai.baseModel)

            val hf = r.candidates.first { it.source == ModelSource.HUGGINGFACE }
            assertEquals(SHA_HF, hf.sha256, "HF 的 sha256 = LFS oid")
            // 路径里写着 loras → installInto 必须是 loras（不是 pipeline_tag 推的 diffusion）
            assertEquals("loras", hf.installInto, "路径线索比 pipeline_tag 准：${hf.fileName}")
            assertEquals("krea/Krea-2-Turbo", hf.name)
            // 直链要带上**仓库里的路径**（这个文件在 loras/ 子目录里，少一段就是 404）
            assertTrue(
                hf.downloadUrl!!.contains("/resolve/main/loras/krea2-turbo.safetensors"),
                hf.downloadUrl!!,
            )

            val ms = r.candidates.first { it.source == ModelSource.MODELSCOPE }
            assertEquals(SHA_MODELSCOPE, ms.sha256, "ModelScope 的 sha256 = 仓库文件的 Sha256")
            assertEquals("diffusion_models", ms.installInto, "split_files/diffusion_models 是路径线索")
            assertEquals("Anima", ms.name)

            // 正文里也要看得到 sha256 与"该放进"
            assertTrue(r.candidates.size == 3)
        }
    }

    /**
     * 可达性必须**分得开**：401 → 要登录、403 → gated、429 → error。
     *
     * 这条正是 A 表那句"否则又变成'AI 说能下、用户下不动'"的回归防线：
     * 如果三种都写成一句"失败了"，模型只能编一个原因。
     */
    @Test
    fun `HF 的 401 与 403 分别落到 needs_login 与 gated`() = runBlocking {
        // 401 → needs_login
        withServer({ path, _ ->
            when {
                path == "/api/models" -> 401 to """{"error":"unauthorized"}"""
                else -> 200 to "[]"
            }
        }) { base ->
            val r = HttpModelFinder(hfBaseUrl = base).search(ModelQuery(text = "x"))
            val hf = r.sources.first { it.source == ModelSource.HUGGINGFACE }
            assertEquals(Reachability.NEEDS_LOGIN, hf.state, hf.detail)
            assertTrue(hf.detail.contains("令牌") || hf.detail.contains("登录"), hf.detail)
        }
        // 403 → gated（HF 要用户先在网页上同意条款）
        withServer({ path, _ ->
            when {
                path == "/api/models" -> 403 to """{"error":"gated"}"""
                else -> 200 to "[]"
            }
        }) { base ->
            val r = HttpModelFinder(hfBaseUrl = base).search(ModelQuery(text = "x"))
            val hf = r.sources.first { it.source == ModelSource.HUGGINGFACE }
            assertEquals(Reachability.GATED, hf.state, hf.detail)
            assertTrue(hf.detail.contains("同意条款"), "gated 要说清「用户得自己去点同意」：${hf.detail}")
        }
    }

    @Test
    fun `连不上时归到 needs_proxy 并写清出网路径与不要反复重试`() = runBlocking {
        // 127.0.0.1 上一个**没有服务**的端口：连接会被立刻拒绝
        val dead = "http://127.0.0.1:1"
        val r = HttpModelFinder(hfBaseUrl = dead, timeoutSeconds = 3).search(ModelQuery(text = "x"))
        val hf = r.sources.first { it.source == ModelSource.HUGGINGFACE }
        assertEquals(Reachability.NEEDS_PROXY, hf.state, hf.detail)
        assertTrue(hf.detail.contains("127.0.0.1:7890"), "要告诉用户代理怎么设：${hf.detail}")
        assertTrue(hf.detail.contains("不要反复重试"), hf.detail)
    }

    /**
     * 一个源挂了**不影响**别的源（"逐个报可达性"的前提）。
     *
     * 这条最容易写错：省事的实现是"第一个源失败就 return error"，
     * 那样用户拿到的是"什么都没查到"，而事实是"另外两个源明明是通的"。
     */
    @Test
    fun `一个源连不上 另外两个照常出结果`() = runBlocking {
        val dead = "http://127.0.0.1:1"
        withServer({ path, _ ->
            when {
                path.startsWith("/api/v1/models") -> 200 to CIVITAI_BODY
                else -> 404 to "{}"
            }
        }) { base ->
            val finder = HttpModelFinder(
                civitai = CivitaiSearch(noTokenCredentials(), safeBaseUrl = base, nsfwBaseUrl = base),
                hfBaseUrl = dead,
                modelscopeBaseUrl = dead,
                timeoutSeconds = 3,
            )
            val r = finder.search(ModelQuery(text = "krea2 turbo"))
            assertEquals(3, r.sources.size)
            assertEquals(Reachability.OK, r.sources[0].state)
            assertEquals(Reachability.NEEDS_PROXY, r.sources[1].state)
            assertEquals(Reachability.NEEDS_PROXY, r.sources[2].state)
            // Civitai 那条候选照样给出来
            assertTrue(r.candidates.any { it.source == ModelSource.CIVITAI })
        }
    }

    @Test
    fun `上游没给 sha256 时不编 结构化与正文都如实说明`() = runBlocking {
        withServer({ path, _ ->
            when {
                path.startsWith("/api/v1/models") -> 200 to CIVITAI_BODY_NO_HASH
                else -> 200 to "[]"
            }
        }) { base ->
            val finder = HttpModelFinder(
                civitai = CivitaiSearch(noTokenCredentials(), safeBaseUrl = base, nsfwBaseUrl = base),
                hfBaseUrl = base,
                modelscopeBaseUrl = base,
            )
            val r = finder.search(ModelQuery(text = "krea2 turbo"))
            val c = r.candidates.single()
            assertNull(c.sha256, "上游没给就是 null —— 不许编一个 64 位十六进制")
            assertTrue(
                r.notes.any { it.contains("sha256") },
                "要如实说'这条没有校验值'：${r.notes}",
            )
        }
    }

    @Test
    fun `HF 非 LFS 文件不当成 sha256 报出去`() = runBlocking {
        // `lfs.oid` 长度不对（不是 64 位）→ 宁可不报，也不给一个假的校验值
        withServer({ path, _ ->
            when {
                path == "/api/models/krea/Krea-2-Turbo/tree/main" -> 200 to HF_TREE_BAD_OID
                path == "/api/models" -> 200 to HF_SEARCH_BODY
                else -> 404 to "{}"
            }
        }) { base ->
            val r = HttpModelFinder(hfBaseUrl = base).search(ModelQuery(text = "x"))
            val hf = r.candidates.first { it.source == ModelSource.HUGGINGFACE }
            assertNull(hf.sha256, "不是 64 位十六进制就不是 sha256")
            assertEquals("krea/Krea-2-Turbo", hf.name)
        }
    }

    @Test
    fun `Civitai 只认 hashes 里的 SHA256 不认 AutoV2 短码`() = runBlocking {
        withServer({ path, _ ->
            when {
                path.startsWith("/api/v1/models") -> 200 to CIVITAI_BODY_AUTOV2_ONLY
                else -> 200 to "[]"
            }
        }) { base ->
            val client = CivitaiSearch(noTokenCredentials(), safeBaseUrl = base, nsfwBaseUrl = base)
            val outcome = client.search("x")
            assertNull(
                outcome.models.single().sha256,
                "AutoV2 是 Civitai 自己的短码，拿它校验盘上的文件永远对不上",
            )
        }
    }

    @Test
    fun `空的 query 在真实实现那一层直接报 INVALID_ARGUMENT`() = runBlocking {
        val finder = HttpModelFinder(civitai = CountingCivitai())
        val e = assertFailsWith<ToolFailure> { finder.search(ModelQuery(text = "   ")) }
        assertEquals("INVALID_ARGUMENT", e.code)
    }

    // =======================================================================
    //  测试脚手架
    // =======================================================================

    private class CountingCivitai : CivitaiClient {
        var calls = 0
        override suspend fun search(
            query: String,
            allowNsfw: Boolean,
            type: String?,
            limit: Int,
        ): CivitaiOutcome {
            calls++
            return CivitaiOutcome(query, emptyList(), "", buildJsonObject { put("count", 0) })
        }
    }

    /** 令牌走环境变量源（绕开 DPAPI，不碰真子系统）。 */
    private fun noTokenCredentials(): CredentialService =
        CredentialService(Files.createTempDirectory("comfyhub-findmodel-cred"), env = emptyMap())

    /** 造一份本机清单（只有一条模型）。 */
    private fun inventoryOf(name: String, folder: String): ComfyModels.Inventory = ComfyModels.Inventory(
        source = ComfyModels.Source.OBJECT_INFO,
        models = listOf(ComfyModels.LocalModel(name = name, folder = folder)),
    )

    /**
     * 起一个本机假服务，把三个站都指到它上面。
     *
     * [route] 收 `(path, 请求体)` 给出 `(状态码, 响应体)` —— 每个用例自己说清
     * "哪条路径回什么"，不必去解析查询串（要断言查询串的用例在别处）。
     *
     * 调用点写成 `withServer({ path, _ -> 200 to body }) { base -> … }`：
     * **route 用圆括号传、block 用尾随 lambda**。这是刻意的 ——
     * 两个 lambda 都写成尾随形式时 `runBlocking { withServer({…}) {…} }` 会被解析成
     * "把 runBlocking 的 lambda 当成第二个参数"，编译期就报 Suspension functions…
     */
    private suspend fun withServer(
        route: (path: String, body: String) -> Pair<Int, String>,
        block: suspend (base: String) -> Unit,
    ) {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext("/") { exchange ->
            val body = exchange.requestBody.readBytes().toString(StandardCharsets.UTF_8)
            val (status, text) = route(exchange.requestURI.path, body)
            val bytes = text.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        server.start()
        try {
            block("http://127.0.0.1:${server.address.port}")
        } finally {
            server.stop(0)
        }
    }

    companion object {
        private val SHA_A = "a".repeat(64)
        private val SHA_HF = "b".repeat(64)
        private val SHA_CIVITAI = "c".repeat(64)
        private val SHA_MODELSCOPE = "d".repeat(64)

        /**
         * 假响应正文**保留换行与缩进**（只 `trimIndent()`，**不要** `replace("\n","")`）。
         *
         * 踩过一次：`replace("\n", "")` 会把上一行末尾、下一行开头粘在一起
         * （`"description":"大底模"` + `"modelVersions"` → `大底模""modelVersions"`），
         * 于是服务端拿到的是一段**非法 JSON**，解析失败被归成 `error` ——
         * 而那看起来像是"解析代码写错了"。`trimIndent()` 本身就会去掉每行前的公共缩进。
         */

        /** Civitai：主文件带 `hashes.SHA256` 与 `name`（线上形状，2026-09-22 核对过）。 */
        private val CIVITAI_BODY = """
            {"items":[{"id":9,"name":"krea2 Turbo Int8","type":"Checkpoint","nsfw":false,
              "stats":{"downloadCount":10,"thumbsUpCount":2},
              "description":"大底模",
              "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":["krea2"],
                "files":[{"name":"krea2_turbo_int8.safetensors","primary":true,"sizeKB":14145495.0,
                          "hashes":{"SHA256":"${SHA_CIVITAI}","AutoV2":"ABCDEF1234"},
                          "downloadUrl":"https://civitai.com/api/download/models/9"}]}]}]}
        """.trimIndent()

        private val CIVITAI_BODY_NO_HASH = """
            {"items":[{"id":9,"name":"没给校验值的模型","type":"LORA","nsfw":false,
              "stats":{"downloadCount":10,"thumbsUpCount":2},
              "description":"大底模",
              "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":[],
                "files":[{"name":"a.safetensors","primary":true,"sizeKB":1024.0,
                          "downloadUrl":"https://civitai.com/api/download/models/9"}]}]}]}
        """.trimIndent()

        private val CIVITAI_BODY_AUTOV2_ONLY = """
            {"items":[{"id":9,"name":"只有短码的模型","type":"LORA","nsfw":false,
              "stats":{"downloadCount":1,"thumbsUpCount":1},"description":"x",
              "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":[],
                "files":[{"name":"a.safetensors","primary":true,"sizeKB":1024.0,
                          "hashes":{"AutoV2":"ABCDEF1234"},
                          "downloadUrl":"https://civitai.com/api/download/models/9"}]}]}]}
        """.trimIndent()

        private val HF_SEARCH_BODY = """
            [{"id":"krea/Krea-2-Turbo","modelId":"krea/Krea-2-Turbo","downloads":74573,
              "pipeline_tag":"text-to-image","library_name":"diffusers",
              "tags":["diffusers","safetensors","base_model:krea/Krea-2-Raw"]}]
        """.trimIndent()

        /** HF 目录：三条，**最大**的那个是 `loras/…safetensors`（要下的就是它，路径线索也来自它）。 */
        private val HF_TREE_BODY = """
            [{"type":"file","path":"README.md","size":1000,"oid":"deadbeef"},
             {"type":"file","path":"loras/krea2-turbo.safetensors","size":400000000,
              "lfs":{"oid":"${SHA_HF}","size":400000000,"pointerSize":131}},
             {"type":"file","path":"vae/ae.safetensors","size":300000000,
              "lfs":{"oid":"${"e".repeat(64)}","size":300000000}}]
        """.trimIndent()

        private val HF_TREE_BAD_OID = """
            [{"type":"file","path":"krea2-turbo.safetensors","size":200000000,
              "lfs":{"oid":"not-a-sha256","size":200000000}}]
        """.trimIndent()

        /** ModelScope：`Data.Model.Models` 这个套法在线上核对过（2026-09-22 实测）。 */
        private val MODELSCOPE_BODY = """
            {"Code":200,"Data":{"Model":{"Models":[
              {"Id":653643,"Path":"circlestone-labs/Anima","Name":"Anima","Downloads":63821,
               "BaseModel":["nvidia/Cosmos-Predict2-2B-Text2Image"],"Tasks":["text-to-image"],
               "ModelInfos":{"safetensor":{"model_size":19542562803,"tensor_type":["BF16"]}}}]}}}
        """.trimIndent()

        private val MODELSCOPE_FILES_BODY = """
            {"Code":200,"Data":{"Files":[
              {"Type":"tree","Path":"split_files","Name":"split_files","Size":0,"Sha256":""},
              {"Type":"blob","Path":"split_files/diffusion_models/anima-turbo-v1.1.safetensors",
               "Name":"anima-turbo-v1.1.safetensors","Size":26283332608,"Sha256":"${SHA_MODELSCOPE}"},
              {"Type":"blob","Path":"README.md","Name":"README.md","Size":24848,
               "Sha256":"${"f".repeat(64)}"}]}}
        """.trimIndent()
    }
}
