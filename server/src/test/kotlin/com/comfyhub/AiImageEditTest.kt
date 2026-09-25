package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.comfyhub.ai.tools.str
import org.junit.jupiter.api.AfterAll
import java.awt.Color
import java.awt.image.BufferedImage
import java.nio.file.Files
import java.nio.file.Path
import javax.imageio.ImageIO
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * 图像编辑的输入准备（2026-09-26）。
 *
 * 盯的是**这条链上真正会静默出错的三件事**：
 *  ① 遮罩的**方向**（白 = 重绘区）—— 反了就会"该保的被重画、该换的一动没动"，
 *     而两条路都能跑完、都出图，从产物上完全看不出来；
 *  ② 遮罩的**尺寸**必须与底图一致（不一致 ComfyUI 会在采样时报一个用户看不懂的错）；
 *  ③ 投放要**幂等**（同一个产物重复投放得到同一个文件名，重跑工作流不会指到一个已经不存在的文件）。
 *
 * 全程不碰真 ComfyUI：ComfyUI 的位置由**设置里的捕获输出目录**推出来（它的兄弟目录 `input`），
 * 测试把它指到临时目录（`AiWorkflowSearch.resolveComfyInputDir` 的第一条来源）。
 */
class AiImageEditTest {

    private fun env(): Pair<Path, Path> {
        // <tmp>/ComfyUI/{output,input} —— resolveComfyInputDir 认的就是"output 的兄弟 input"
        val home = Files.createTempDirectory("comfyhub-image-edit")
        val output = home.resolve("ComfyUI").resolve("output")
        val input = home.resolve("ComfyUI").resolve("input")
        Files.createDirectories(output)
        Files.createDirectories(input)
        return home to input
    }

    private fun cfgFor(root: Path): AppConfig = AppConfig(
        host = "127.0.0.1",
        port = 0,
        jdbcUrl = TestDb.jdbcUrl,
        dbUser = "",
        dbPassword = "",
        storageDir = root.resolve("storage"),
        maxUploadBytes = 1,
        projectRoot = root,
    )

    private fun png(path: Path, w: Int, h: Int, color: Color) {
        val img = BufferedImage(w, h, BufferedImage.TYPE_INT_RGB)
        val g = img.createGraphics()
        g.color = color
        g.fillRect(0, 0, w, h)
        g.dispose()
        ImageIO.write(img, "png", path.toFile())
    }

    /** 灰度遮罩在 (x,y) 处的值（0=黑=保留，255=白=重绘）。 */
    private fun sample(mask: Path, x: Int, y: Int): Int =
        ImageIO.read(mask.toFile()).raster.getSample(x, y, 0)

    private fun args(vararg pairs: Pair<String, Any?>): JsonObject = buildJsonObject {
        pairs.forEach { (k, v) ->
            when (v) {
                null -> Unit
                is Int -> put(k, v)
                is Boolean -> put(k, v)
                else -> put(k, v.toString())
            }
        }
    }

    @Test
    fun `遮罩按底图尺寸生成 白区就是重绘区`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        // 把捕获输出目录指到临时目录 —— resolveComfyInputDir 的第一条来源
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))
        png(input.resolve("bg.png"), 400, 200, Color.BLUE)

        val out = AiImageEdit.makeMask(
            cfg,
            args("baseFilename" to "bg.png", "region" to "0.25,0.0,0.5,1.0", "feather" to 0),
        )
        assertEquals(400, out.str("width")!!.toInt())
        assertEquals(200, out.str("height")!!.toInt())
        assertEquals("要重新生成的地方（重绘区）", out.str("whiteMeans"))

        val mask = input.resolve(out.str("filename")!!)
        assertTrue(Files.isRegularFile(mask), "遮罩要落在 ComfyUI 的 input 目录：$mask")
        // 中间（x=200）白、两侧黑；边界按比例落在 100 与 300
        assertEquals(255, sample(mask, 200, 100), "region 里必须是白（要重绘）")
        assertEquals(0, sample(mask, 20, 100), "region 外必须是黑（原样保留）")
        assertEquals(0, sample(mask, 380, 100))
        assertEquals(255, sample(mask, 120, 5))
    }

    @Test
    fun `invert 反过来 换背景保人物靠它`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))
        png(input.resolve("bg.png"), 200, 200, Color.BLUE)

        val out = AiImageEdit.makeMask(
            cfg,
            args("baseFilename" to "bg.png", "region" to "center", "invert" to true),
        )
        val mask = input.resolve(out.str("filename")!!)
        // center = (0.25,0.1,0.5,0.8)：中心那块被反过来变成黑（保住人物），四角变白（重绘背景）
        assertEquals(0, sample(mask, 100, 100), "反选之后人物那块必须是黑（保住）")
        assertEquals(255, sample(mask, 5, 5), "反选之后背景必须是白（重绘）")
        assertEquals("true", out.str("invert"))
    }

    @Test
    fun `羽化让边缘出现中间灰度 而不是硬边`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))
        png(input.resolve("bg.png"), 200, 200, Color.BLUE)

        val out = AiImageEdit.makeMask(
            cfg,
            args("baseFilename" to "bg.png", "region" to "left", "feather" to 24),
        )
        val mask = input.resolve(out.str("filename")!!)
        // left = 左半白：边界在 x=100。羽化后边界附近应当出现 0 < v < 255
        val edge = (95..105).map { sample(mask, it, 100) }
        assertTrue(edge.any { it in 20..235 }, "羽化要产生中间灰度（实测 $edge）")
        assertEquals(255, sample(mask, 10, 100))
        assertEquals(0, sample(mask, 190, 100))
    }

    @Test
    fun `同样的参数得到同一个文件名 不同参数不会互相覆盖`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))
        png(input.resolve("bg.png"), 100, 100, Color.BLUE)

        val a = AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "region" to "full")).str("filename")
        val b = AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "region" to "full")).str("filename")
        val c = AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "region" to "left")).str("filename")
        assertEquals(a, b, "同一组参数必须得到同一个文件（重复调用不堆垃圾）")
        assertTrue(a != c, "参数变了就该是另一个文件")
        val masks = Files.list(input).use { s ->
            s.filter { it.fileName.toString().startsWith("comfyhub_mask_") }.count()
        }
        assertEquals(2L, masks, "两次不同参数只该落两个遮罩文件（同名调用不堆垃圾）")
    }

    @Test
    fun `参数不合法一律报人话 不猜也不静默`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))
        png(input.resolve("bg.png"), 100, 100, Color.BLUE)

        // 底图不在 input 里 → 要说清"先投放底图"，而不是去猜一个尺寸
        val e1 = assertFailsWith<com.comfyhub.ai.tools.ToolFailure> {
            AiImageEdit.makeMask(cfg, args("baseFilename" to "nope.png"))
        }
        assertTrue(e1.message!!.contains("comfy_use_media"), e1.message!!)
        // region 文案不认识 → 列出支持的写法
        val e2 = assertFailsWith<com.comfyhub.ai.tools.ToolFailure> {
            AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "region" to "中间那个人"))
        }
        assertTrue(e2.message!!.contains("x,y,w,h"), e2.message!!)
        // 归一化坐标越界
        assertFailsWith<com.comfyhub.ai.tools.ToolFailure> {
            AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "region" to "10,0,0.5,0.5"))
        }
        // 目录穿越
        val out = AiImageEdit.makeMask(cfg, args("baseFilename" to "bg.png", "filename" to "..\\..\\evil.png"))
        assertTrue(!out.str("filename")!!.contains(".."), "文件名只取裸名：${out.str("filename")}")
    }

    @Test
    fun `画廊产物投放进 input 目录 返回真实文件名与原图尺寸 且重复投放幂等`() {
        TestDb.reset()
        val (root, input) = env()
        val cfg = cfgFor(root)
        SettingsRepo.saveCaptureConfig(CaptureConfig(outputDir = input.parent.resolve("output").toString()))

        // 造一条真实的产物：库里有行、盘上有文件
        val mediaDir = root.resolve("storage").resolve("media")
        Files.createDirectories(mediaDir)
        png(mediaDir.resolve("stored-1.png"), 320, 480, Color.GREEN)
        val mediaId = Db.tx { conn ->
            conn.executeReturningKey(
                "INSERT INTO media_assets (kind, title, original_name, stored_name) VALUES ('IMAGE','人物图','hero.png','stored-1.png')"
            )
        }

        val first = AiImageEdit.useMedia(cfg, mediaDir, mediaId, null)
        val filename = first.str("filename")!!
        assertTrue(filename.startsWith("hero-"), "名字要能看出是哪个产物：$filename")
        assertTrue(Files.isRegularFile(input.resolve(filename)))
        assertEquals("320", first.str("width"))
        assertEquals("480", first.str("height"))

        // 再投一次：同一个文件名（重跑工作流不会指向已经不在的文件）
        val again = AiImageEdit.useMedia(cfg, mediaDir, mediaId, null)
        assertEquals(filename, again.str("filename"))
        assertEquals(1, Files.list(input).use { it.count() })

        // 非图片产物要说清（视频不能当底图）
        val videoId = Db.tx { conn ->
            conn.executeReturningKey(
                "INSERT INTO media_assets (kind, title, original_name, stored_name) VALUES ('VIDEO','视频','v.mp4','v.mp4')"
            )
        }
        val e = assertFailsWith<com.comfyhub.ai.tools.ToolFailure> { AiImageEdit.useMedia(cfg, mediaDir, videoId, null) }
        assertEquals("NOT_AN_IMAGE", e.code)
    }

    companion object {
        @AfterAll
        @JvmStatic
        fun cleanup() = TestDb.cleanup()
    }
}
