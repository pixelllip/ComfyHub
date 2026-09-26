package com.comfyhub

import com.comfyhub.ai.tools.ToolFailure
import com.comfyhub.ai.tools.bool
import com.comfyhub.ai.tools.int
import com.comfyhub.ai.tools.str
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.awt.Color
import java.awt.RenderingHints
import java.awt.image.BufferedImage
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import javax.imageio.ImageIO
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * 图像编辑的**输入准备**（2026-09-26，用户需求："保持背景不变，更换有具体动作的人物并保持画风"
 * + "换背景保留人物"）。
 *
 * 这一层只做两件事，都落在 **ComfyUI 的 `input` 目录**里（`LoadImage` / `LoadImageMask`
 * 只能读那里面的文件）：
 *
 * 1. [useMedia] —— 把**画廊里的产物**（阶段一生成的人物图、要保留的背景图…）投放进去，
 *    拿到它在那边的真实文件名。它和 `comfy_use_attachment`（用户发来的附件）是**同一个池子**，
 *    只是来源不同：那条走 `<storage>/ai-attachments`，这条走 `<storage>/media`。
 *    为什么非要有它：两阶段流程里"阶段一的产物"天然在画廊，而不在聊天附件里。
 * 2. [makeMask] —— **生成遮罩**。局部重绘（inpaint）必须有遮罩，而遮罩从哪来是个真问题：
 *    用户在手机 / 平板上画不出来，ComfyUI 的 MaskEditor 又要人坐在电脑前手描。
 *    这里用"归一化矩形 / 椭圆 + 反选 + 羽化"表达意图，够用且可复现。
 *
 * ## 遮罩的语义（**白 = 要重新生成的地方**）
 *
 * 这与 ComfyUI 的约定一致（`SetLatentNoiseMask` / `LatentComposite` 都认这个方向），
 * 所以两种需求**用的是同一个工作流**，只是遮罩反过来：
 *
 * | 需求 | region | invert | 意思 |
 * | --- | --- | --- | --- |
 * | **保背景换人物** | 人物所在的那块 | false | 只重绘人物 |
 * | **换背景保人物** | 人物所在的那块 | **true** | 人物之外全部重绘 |
 * | 整张重画（等价图生图） | `full` | false | 全画面 |
 *
 * ⚠ `region` 是**近似**的：工具不会做分割，它只是照给的框画。所以工具结果里会回
 * `approximate=true` 与它实际用的框，模型必须如实转述"框是我估的、不准就报新坐标"。
 */
object AiImageEdit {

    /** 遮罩边长上限（与产物同一量级；再大就是误传了）。 */
    const val MAX_MASK_EDGE = 8192

    /** 羽化上限（像素）。 */
    const val MAX_FEATHER = 256

    /**
     * 把画廊里的一个**图片产物**放进 ComfyUI 的 `input` 目录。
     *
     * 返回 `{mediaId, filename, inputDir, width, height, hint}` —— `width/height` 是
     * **原图尺寸**，做遮罩时要照它给（遮罩与底图尺寸不一致 = 采样直接炸）。
     */
    fun useMedia(cfg: AppConfig, mediaDir: Path, mediaId: Long, wantedName: String?): JsonObject {
        val media = MediaRepo.get(mediaId)
            ?: throw ToolFailure("MEDIA_NOT_FOUND", "画廊里没有 id=$mediaId 的产物（先用 gallery / media 列表确认它还在）")
        if (!media.kind.equals("IMAGE", ignoreCase = true)) {
            throw ToolFailure(
                "NOT_AN_IMAGE",
                "产物 #$mediaId「${media.title}」是 ${media.kind}，不是图片。" +
                    "只有图片能放进 ComfyUI 的 input 目录当底图 / 参考图 / 遮罩。",
            )
        }
        val source = mediaDir.resolve(media.storedName)
        if (!Files.isRegularFile(source)) {
            throw ToolFailure("MEDIA_NOT_FOUND", "产物 #$mediaId 的原件不在盘上了：$source")
        }
        val inputDir = AiWorkflowSearch.resolveComfyInputDir(cfg)
            ?: throw ToolFailure(
                "COMFY_NOT_FOUND",
                "没找到 ComfyUI 的目录，没法把图片放进它的 input 目录。" +
                    "让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置后再试。",
            )

        // 名字里带 mediaId 的前 8 位：同一个产物永远同一个名字（重跑不会指向已经不在的文件），
        // 不同产物即使标题相同也不会互相覆盖（与 comfy_use_attachment 同一约定）
        val filename = AiWorkflowSearch.inputFilenameFor(media.originalName, "m$mediaId", wantedName)
        copyIfNeeded(source, inputDir.resolve(filename))

        // 尺寸读不出来不拦（遮罩那一步会拿不到 baseFilename 的尺寸、由模型显式给）——
        // 但要在结果里如实说：`width`/`height` 只在这张图能解码时才有
        val size = readSize(source)
        return buildJsonObject {
            put("mediaId", mediaId)
            put("filename", filename)
            put("inputDir", inputDir.toString())
            size?.let {
                put("width", it.first)
                put("height", it.second)
            }
            put(
                "hint",
                "用 comfy_submit 覆盖工作流里 LoadImage 节点的 image 输入：" +
                    """{"<节点id>.image":"$filename"}；""" +
                    "要遮罩就用 comfy_make_mask(baseFilename=\"$filename\", region=…) —— " +
                    "它会照这张图的尺寸生成遮罩。",
            )
        }
    }

    /**
     * 生成一张遮罩 PNG 放进 ComfyUI 的 `input` 目录（**白 = 要重新生成的地方**）。
     *
     * 参数（都从模型给的 JSON 里取，类型一律容错）：
     *  - `baseFilename`：input 目录里的底图文件名 —— 给了就用**它的尺寸**（推荐，最不容易错）；
     *  - `width` / `height`：没给 `baseFilename` 时的显式尺寸；
     *  - `region`：`full`（默认）/ `x,y,w,h`（归一化 0~1）/ 预设 `left|right|top|bottom|center|person`；
     *  - `shape`：`rect`（默认）/ `ellipse`；
     *  - `invert`：反选（**换背景保人物**就是 `invert=true` + 人物那块 region）；
     *  - `feather`：边缘羽化像素（默认 0；贴人物进新背景时 8~24 会明显更自然）；
     *  - `filename`：想固定的文件名（不给自己按内容算一个，重复调用得到同一个文件）。
     */
    fun makeMask(cfg: AppConfig, args: JsonObject): JsonObject {
        val inputDir = AiWorkflowSearch.resolveComfyInputDir(cfg)
            ?: throw ToolFailure(
                "COMFY_NOT_FOUND",
                "没找到 ComfyUI 的 input 目录（遮罩必须放在那里，LoadImageMask 才读得到）。" +
                    "让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置。",
            )

        val baseFilename = args.str("baseFilename")
        var width = args.int("width")
        var height = args.int("height")
        if (!baseFilename.isNullOrBlank()) {
            val base = inputDir.resolve(fileNameOnly(baseFilename))
            if (!Files.isRegularFile(base)) {
                throw ToolFailure(
                    "INVALID_ARGUMENT",
                    "input 目录里没有「$baseFilename」。先用 comfy_use_media（画廊产物）或 " +
                        "comfy_use_attachment（用户发来的图）把底图投放进去，再照它生成遮罩。",
                )
            }
            val size = readSize(base) ?: throw ToolFailure(
                "INVALID_ARGUMENT",
                "读不出「$baseFilename」的像素尺寸（可能不是 PNG/JPG）：遮罩尺寸必须与底图一致。",
            )
            width = size.first
            height = size.second
        }
        val w = width ?: throw ToolFailure("INVALID_ARGUMENT", "要么给 baseFilename，要么给 width 与 height")
        val h = height ?: throw ToolFailure("INVALID_ARGUMENT", "要么给 baseFilename，要么给 width 与 height")
        if (w < 16 || h < 16 || w > MAX_MASK_EDGE || h > MAX_MASK_EDGE) {
            throw ToolFailure("INVALID_ARGUMENT", "尺寸不合法：${w}x$h（要在 16~$MAX_MASK_EDGE 之间）")
        }

        val regionText = args.str("region") ?: "full"
        val region = parseRegion(regionText)
        val shape = (args.str("shape") ?: "rect").lowercase()
        if (shape != "rect" && shape != "ellipse") {
            throw ToolFailure("INVALID_ARGUMENT", "shape 只支持 rect / ellipse，收到「$shape」")
        }
        val invert = args.bool("invert") ?: false
        val feather = (args.int("feather") ?: 0).coerceIn(0, MAX_FEATHER)

        val mask = BufferedImage(w, h, BufferedImage.TYPE_BYTE_GRAY)
        val g = mask.createGraphics()
        g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
        g.color = Color.BLACK
        g.fillRect(0, 0, w, h)
        g.color = Color.WHITE
        val x = (region[0] * w).roundToInt().coerceIn(0, w)
        val y = (region[1] * h).roundToInt().coerceIn(0, h)
        val rw = (region[2] * w).roundToInt().coerceIn(0, w - x)
        val rh = (region[3] * h).roundToInt().coerceIn(0, h - y)
        if (rw > 0 && rh > 0) {
            if (shape == "ellipse") g.fillOval(x, y, rw, rh) else g.fillRect(x, y, rw, rh)
        }
        g.dispose()
        if (invert) invertInPlace(mask)
        if (feather > 0) boxBlur(mask, feather)

        val filename = args.str("filename")?.takeIf { it.isNotBlank() }?.let { fileNameOnly(it) }
            ?: "comfyhub_mask_${contentTag(w, h, region, shape, invert, feather)}.png"
        val target = inputDir.resolve(filename)
        try {
            Files.createDirectories(inputDir)
            ImageIO.write(mask, "png", target.toFile())
        } catch (e: Exception) {
            throw ToolFailure("COMFY_INPUT_FAILED", "遮罩写不进 ${inputDir}：${e.message?.take(200)}")
        }

        return buildJsonObject {
            put("filename", filename)
            put("inputDir", inputDir.toString())
            put("width", w)
            put("height", h)
            put("region", regionText)
            put("regionNormalized", "[${region.joinToString(",")}]")
            put("shape", shape)
            put("invert", invert)
            put("feather", feather)
            put("whiteMeans", "要重新生成的地方（重绘区）")
            put(
                "approximate",
                regionText != "full" && !regionText.contains(','),
            )
            put(
                "hint",
                "① 覆盖工作流里 LoadImageMask 节点的 image 输入：{\"<节点id>.image\":\"$filename\"}；" +
                    "② 若工作流有 ImageScale，把 width/height 也覆盖成 ${w}x$h（必须与底图一致）；" +
                    "③ 遮罩不准就报一组新的 region（x,y,w,h 归一化）重做 —— 不要让用户去 ComfyUI 里手画。",
            )
        }
    }

    // -----------------------------------------------------------------------
    //  内部
    // -----------------------------------------------------------------------

    /** 只取文件名（挡 `..\` 之类的目录穿越；ComfyUI 也只认裸文件名）。 */
    private fun fileNameOnly(raw: String): String =
        raw.replace('\\', '/').substringAfterLast('/').trim().take(160)

    private fun copyIfNeeded(source: Path, target: Path) {
        try {
            Files.createDirectories(target.parent)
            val same = Files.isRegularFile(target) && Files.size(target) == Files.size(source)
            if (!same) Files.copy(source, target, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: Exception) {
            throw ToolFailure("COMFY_INPUT_FAILED", "写不进 ComfyUI 的 input 目录（${target.parent}）：${e.message?.take(200)}")
        }
    }

    /** 原图尺寸（读不出来回 null —— 调用方按"如实报错"处理）。 */
    private fun readSize(file: Path): Pair<Int, Int>? = runCatching {
        ImageIO.read(file.toFile())?.let { it.width to it.height }
    }.getOrNull()

    /**
     * `region` 解析成归一化的 `[x, y, w, h]`。
     *
     * 四个预设都是"粗框"，够表达"左边那个人 / 中间那个人"：
     * `center` 与 `person` 是竖长方框（人物通常站着），`left` / `right` / `top` / `bottom` 是半张。
     */
    private fun parseRegion(raw: String): DoubleArray {
        val text = raw.trim().lowercase()
        when (text) {
            "full", "all", "整张", "" -> return doubleArrayOf(0.0, 0.0, 1.0, 1.0)
            "left" -> return doubleArrayOf(0.0, 0.0, 0.5, 1.0)
            "right" -> return doubleArrayOf(0.5, 0.0, 0.5, 1.0)
            "top" -> return doubleArrayOf(0.0, 0.0, 1.0, 0.5)
            "bottom" -> return doubleArrayOf(0.0, 0.5, 1.0, 0.5)
            "center", "centre" -> return doubleArrayOf(0.25, 0.1, 0.5, 0.8)
            "person" -> return doubleArrayOf(0.22, 0.05, 0.56, 0.92)
        }
        val parts = text.split(',').map { it.trim() }.filter { it.isNotEmpty() }
        if (parts.size != 4) {
            throw ToolFailure(
                "INVALID_ARGUMENT",
                "region 只支持 full / left / right / top / bottom / center / person，" +
                    "或者四个归一化数字 `x,y,w,h`（例如 \"0.3,0.05,0.4,0.9\"）。收到「$raw」",
            )
        }
        val nums = parts.map {
            it.toDoubleOrNull() ?: throw ToolFailure("INVALID_ARGUMENT", "region 里的「$it」不是数字")
        }
        if (nums.any { it < 0.0 || it > 1.0 }) {
            throw ToolFailure("INVALID_ARGUMENT", "region 的四个数都是**归一化**的（0~1），收到「$raw」")
        }
        val x = nums[0]
        val y = nums[1]
        val w = min(nums[2], 1.0 - x)
        val h = min(nums[3], 1.0 - y)
        if (w <= 0.0 || h <= 0.0) throw ToolFailure("INVALID_ARGUMENT", "region 的宽高必须大于 0（收到「$raw」）")
        return doubleArrayOf(x, y, w, h)
    }

    private fun invertInPlace(image: BufferedImage) {
        for (y in 0 until image.height) {
            for (x in 0 until image.width) {
                val v = image.raster.getSample(x, y, 0)
                image.raster.setSample(x, y, 0, 255 - v)
            }
        }
    }

    /** 两次一维均值模糊（可分离），半径 = [feather]/2。 */
    private fun boxBlur(image: BufferedImage, feather: Int) {
        val radius = max(1, feather / 2)
        val w = image.width
        val h = image.height
        val src = IntArray(w * h)
        for (y in 0 until h) for (x in 0 until w) src[y * w + x] = image.raster.getSample(x, y, 0)
        val tmp = IntArray(w * h)
        // 横向
        for (y in 0 until h) {
            var sum = 0
            val row = y * w
            for (i in -radius..radius) sum += src[row + i.coerceIn(0, w - 1)]
            for (x in 0 until w) {
                tmp[row + x] = sum / (2 * radius + 1)
                val out = (x - radius).coerceIn(0, w - 1)
                val inn = (x + radius + 1).coerceIn(0, w - 1)
                sum += src[row + inn] - src[row + out]
            }
        }
        // 纵向
        for (x in 0 until w) {
            var sum = 0
            for (i in -radius..radius) sum += tmp[i.coerceIn(0, h - 1) * w + x]
            for (y in 0 until h) {
                val v = sum / (2 * radius + 1)
                image.raster.setSample(x, y, 0, v.coerceIn(0, 255))
                val out = (y - radius).coerceIn(0, h - 1)
                val inn = (y + radius + 1).coerceIn(0, h - 1)
                sum += tmp[inn * w + x] - tmp[out * w + x]
            }
        }
    }

    /** 参数指纹（同一组参数 → 同一个文件名，重复调用不会在 input 目录里堆垃圾）。 */
    private fun contentTag(
        w: Int,
        h: Int,
        region: DoubleArray,
        shape: String,
        invert: Boolean,
        feather: Int,
    ): String {
        val text = "$w|$h|${region.joinToString(",")}|$shape|$invert|$feather"
        val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray())
        return digest.take(6).joinToString("") { "%02x".format(it) }
    }
}
