package com.comfyhub

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

/**
 * **后端实际监听的端口**（2026-09-22，用户要求"不再显式指定端口、启动时自动挑一个"）。
 *
 * ## 为什么需要这个文件
 *
 * 过去端口写死在好几处：`scripts\comfyhub.ps1`（探测用）、`scripts\server.ps1`（启动用）、
 * `Config.kt`（默认值）、`lib\core\settings_store.dart`（App 连谁）、
 * `comfyui\comfyhub_capture\capture_core.py`（ComfyUI 节点往哪推）。**而且它们互不知道** ——
 * `comfyhub.ps1` 从来没把 `-Port` 透传给 `server.ps1`，所以只要有人改了一处，就会出现
 * "后端明明在跑、脚本却说没起来"这种自相矛盾的现场。
 *
 * 本机 2026-09-22 还撞上一个真实的坏情况：**8080 落在 Windows 的保留端口段里**
 * （Hyper-V/WSL 预留，`netsh int ipv4 show excludedportrange protocol=tcp` 能看到），
 * 于是"端口被占"这件事连 `netstat` 都查不出来，后端一启动就是
 * `BindException: Address already in use`。
 *
 * ## 做法：让**后端自己**决定，然后把它记在一个双方都读的地方
 *
 * 1. 候选端口 [AppConfig.port] 为 **0** 时，交给操作系统挑一个空闲端口（`port = 0` 的语义）；
 * 2. 候选端口被占 / 落在保留段 → **不报错退出**，改成 0 再绑一次（用户要求"失败了回到随机端口"）；
 * 3. 绑定成功后把**真实端口**写进 `<项目根>\.run\api.json`，并原子替换；
 * 4. 前端（`settings_store.dart`）、脚本（`scripts\api-port.ps1`）、
 *    ComfyUI 捕获扩展（`capture_core.py`）都读这**同一个文件** —— 端口只有一个真源。
 *
 * ## 这个文件的纪律
 *
 *  - **只由后端写**（谁在听谁知道）。脚本与前端**只读**，绝不自己编一个端口写进去；
 *  - 写入是**原子的**（先写 `.tmp` 再 `move` 覆盖）：读侧任何时刻看到的要么是旧的完整内容、
 *    要么是新的完整内容，不会读到半截 JSON；
 *  - 后端**优雅退出时删掉它**，但删之前要确认"这文件还是我写的那一份"（`pid` 对得上）——
 *    否则重启场景下新进程刚写的文件会被旧进程的收尾删掉；
 *  - 文件里**不含任何机密**（只有端口 / PID / 时间 / URL），所以放在 `.run\` 里是安全的。
 */
@Serializable
data class ApiPortState(
    /** 后端实际监听的端口 */
    val port: Int,
    /** 写这个文件的后端进程 PID（脚本据此判断"是不是同一个后端"） */
    val pid: Long = 0,
    /** 监听地址（默认回环；开了 `COMFYHUB_ALLOW_REMOTE` 时是 `0.0.0.0`） */
    val host: String = "127.0.0.1",
    /** 给"人"看的完整地址（脚本与扩展直接拿去用，不必自己拼） */
    val url: String = "",
    /** 写文件的时刻（ISO-8601，排查"这个文件是不是几天前的"） */
    val startedAt: String = "",
    /**
     * 用户/脚本**指定**的候选端口，还是自动挑的。
     *
     * 用处只有一个：`doctor` / 启动日志能如实说清"你给的 8280 被占了，我换成了 51234"，
     * 而不是让用户对着一个"我明明写了 8280"的设置发呆。
     */
    val requestedPort: Int = 0,
    /** 是不是从候选端口回落到随机端口的（true = 候选端口没绑上） */
    val fellBack: Boolean = false,
) {
    fun toJson(): JsonObject = buildJsonObject {
        put("port", port)
        put("pid", pid)
        put("host", host)
        put("url", url)
        put("startedAt", startedAt)
        put("requestedPort", requestedPort)
        put("fellBack", fellBack)
    }
}

object ApiPortFile {

    private val log = LoggerFactory.getLogger(ApiPortFile::class.java)

    /** 文件名固定：脚本 / 前端 / Python 扩展三边都按这个名字找。 */
    const val FILE_NAME = "api.json"

    /** `.run` 目录名（与 `scripts\*.ps1` 的 `$RunDir` 约定一致）。 */
    const val RUN_DIR = ".run"

    /** 状态文件在项目根下的位置。 */
    fun path(projectRoot: Path): Path = projectRoot.resolve(RUN_DIR).resolve(FILE_NAME)

    /**
     * 原子写入（写 `.tmp` → 覆盖）。
     *
     * 为什么必须原子：前端与脚本是**轮询**读这个文件的（等后端起来的那几秒里会读好几遍），
     * 直接 `writeText` 会让它们有机会读到半个 JSON —— 那会变成"启动页偶尔连不上"的幽灵 bug。
     */
    fun write(projectRoot: Path, state: ApiPortState): Path? = try {
        val target = path(projectRoot)
        Files.createDirectories(target.parent)
        val tmp = target.resolveSibling("$FILE_NAME.tmp")
        Files.writeString(tmp, AppJson.encodeToString(ApiPortState.serializer(), state))
        // ⚠️ Windows 上"替换一个正被别的进程打开的文件"会直接 IOException
        // （restart 期间上一个后端可能还没退干净、或安全软件正在扫这个文件）。
        // 端口记录**必须**写下去（读端全靠它），所以这里带退避重试几次，
        // 而不是失败一次就算了 —— 2026-09-22 实测：写失败会让"自动端口"整条链路
        // 看起来像卡住（前端连不上、脚本等到超时）。
        var last: Exception? = null
        repeat(5) { i ->
            val ok = runCatching {
                Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
            }.recoverCatching {
                // 个别文件系统（网络盘 / FAT）不支持 ATOMIC_MOVE：退回普通覆盖
                Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING)
            }
            if (ok.isSuccess) {
                log.info("已记录后端端口：{}（port={} pid={}{}）", target, state.port, state.pid, if (i > 0) "，重试 $i 次" else "")
                return target
            }
            last = ok.exceptionOrNull() as? Exception
            Thread.sleep(200L * (i + 1))
        }
        log.warn("写端口状态文件失败（试了 5 次）：{}", last?.message)
        null
    } catch (e: Exception) {
        // 写不了不该让后端起不来：端口记不下最多让前端多试几次，服务本身照常可用
        log.warn("写端口状态文件失败（忽略，服务照常运行）：{}", e.message)
        null
    }

    /**
     * 删除状态文件 —— **只有确认它还是本进程写的那一份才删**。
     *
     * 场景：用户点「重启」，新的后端已经起来并写好了新文件，这时旧进程的收尾才跑完；
     * 无条件删会把**新进程的**记录删掉，前端随后就再也找不到后端了。
     */
    fun deleteIfOwned(projectRoot: Path, pid: Long): Boolean = try {
        val target = path(projectRoot)
        val mine = read(projectRoot)?.pid == pid
        if (!mine) {
            log.info("端口状态文件不属于本进程（pid 不匹配），保留不删：{}", target)
            false
        } else {
            Files.deleteIfExists(target)
            log.info("已清理端口状态文件：{}", target)
            true
        }
    } catch (e: Exception) {
        log.warn("清理端口状态文件失败（忽略）：{}", e.message)
        false
    }

    /** 读出来（读不到 / 格式坏了都返回 null，绝不抛）。 */
    fun read(projectRoot: Path): ApiPortState? = try {
        val target = path(projectRoot)
        if (!Files.isRegularFile(target)) {
            null
        } else {
            runCatching {
                AppJson.decodeFromString(ApiPortState.serializer(), Files.readString(target))
            }.getOrNull()
        }
    } catch (e: Exception) {
        null
    }

    /**
     * 拼一个"能连上"的地址。
     *
     * `COMFYHUB_HOST` 显式给了非回环地址（局域网 / 远程）时，对**本机**客户端来说
     * `0.0.0.0` 这种"全部网卡"地址是连不上的 —— 那种情况回落到 `127.0.0.1`，
     * 否则 App 会拿着一个永远连不上的 URL 干等。
     */
    fun urlFor(host: String, port: Int): String {
        val connectHost = when (host) {
            "0.0.0.0", "::", "[::]", "" -> "127.0.0.1"
            else -> host
        }
        val shown = if (connectHost.contains(':') && !connectHost.startsWith("[")) "[$connectHost]" else connectHost
        return "http://$shown:$port"
    }
}
