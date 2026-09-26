package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import java.nio.file.Files
import java.nio.file.Path

/**
 * 「把 ComfyUI Desktop 的共享模型根默认挂上」（用户 2026-09-22 拍板）。
 *
 * 现场（会话 `0bedb3f5`）的实据：`launch_comfyui` 直接跑 `main.py`，Desktop 的
 * `--extra-model-paths-config` / `--input-directory` / `--output-directory` 一个都没传，
 * 于是实例的 `diffusion_models\` 只有 2 个 minimax、`input` 是它自己的 input，
 * AI 据此得出"本机没有 `anima-base-v1.0`"——**错的**，文件就在
 * `D:\Comfy-Desktop\ComfyUI-Shared\models` 与 `E:\ComfyUI\models` 里。
 *
 * 这些用例钉住四件事：
 *  ① 能从 Desktop 自己的配置里读出**全部**模型根（本机实测有两个，不是一个）；
 *  ② 补参数时**不覆盖**模型自己写的参数；
 *  ③ 目录不存在时**不加**那个参数（给 ComfyUI 一个不存在的目录，启动会直接失败）；
 *  ④ 拿不到配置时**如实警告**，而不是假装环境是好的。
 */
class ComfyEnvTest {

    private fun tmp(): Path = Files.createTempDirectory("comfyhub-env").toRealPath()

    private fun sharedModels(root: Path) = root.resolve("ComfyUI-Shared").resolve("models")
    private fun otherModels(root: Path) = root.resolve("Other").resolve("models")
    private fun sharedInput(root: Path) = root.resolve("ComfyUI-Shared").resolve("input")
    private fun sharedOutput(root: Path) = root.resolve("ComfyUI-Shared").resolve("output")

    /** 造一个"ComfyUI Desktop 装过"的配置目录：两个模型根、共享 input/output。 */
    private fun desktopDir(root: Path, withSharedYaml: Boolean = true): Path {
        val desk = root.resolve("Comfy Desktop")
        Files.createDirectories(desk)
        Files.createDirectories(root.resolve("ComfyUI-Installs"))
        listOf(sharedModels(root), otherModels(root), sharedInput(root), sharedOutput(root))
            .forEach { Files.createDirectories(it) }

        Files.writeString(
            desk.resolve("settings.json"),
            buildJsonObject {
                put("modelsDirs", buildJsonArray {
                    add(JsonPrimitive(sharedModels(root).toString()))
                    add(JsonPrimitive(otherModels(root).toString()))
                })
                put("inputDir", sharedInput(root).toString())
                put("outputDir", sharedOutput(root).toString())
                put("installDir", root.resolve("ComfyUI-Installs").toString())
            }.toString(),
        )
        if (withSharedYaml) {
            Files.writeString(
                desk.resolve("shared_model_paths.yaml"),
                "comfy.desktop_0:\n  base_path: '${sharedModels(root)}'\n  is_default: true\n",
            )
        }
        return desk
    }

    @Test
    fun `读得出两个模型根与共享的 input output`() {
        val root = tmp()
        val desk = desktopDir(root)

        val env = assertNotNull(ComfyEnv.desktop(desk), "有 settings.json 就该读出来")
        assertEquals(
            listOf(sharedModels(root), otherModels(root)),
            env.modelsDirs,
            "本机实测两个模型根，只认一个就是错的",
        )
        assertEquals(sharedInput(root).toString(), env.inputDir.toString())
        assertEquals(sharedOutput(root).toString(), env.outputDir.toString())
        assertEquals(desk.resolve("shared_model_paths.yaml").toString(), env.modelPathsConfig.toString())
    }

    @Test
    fun `没有 shared yaml 时退回实例级 yaml`() {
        val root = tmp()
        val desk = desktopDir(root, withSharedYaml = false)
        val inst = desk.resolve("instance-model-paths")
        Files.createDirectories(inst)
        Files.writeString(inst.resolve("inst-1.yaml"), "comfy.desktop_0:\n  base_path: 'X'\n")

        val env = assertNotNull(ComfyEnv.desktop(desk))
        assertEquals(inst.resolve("inst-1.yaml").toString(), env.modelPathsConfig.toString())
    }

    @Test
    fun `没装 Desktop 时读不到 不报异常`() {
        val root = tmp()
        assertNull(ComfyEnv.desktop(root.resolve("nope")))
        assertNull(ComfyEnv.desktop(null))
        // 有目录但两份配置都没有：也是 null（不是"半个空对象"）
        val empty = root.resolve("Empty")
        Files.createDirectories(empty)
        assertNull(ComfyEnv.desktop(empty))
    }

    @Test
    fun `模型没写参数时补上三件 并把 extra_args 归一成数组`() {
        val root = tmp()
        val env = ComfyEnv.desktop(desktopDir(root))

        val plan = ComfyEnv.prepareLaunch(buildJsonObject { }, env)
        val argv = plan.args["extra_args"].asStrings()

        assertTrue(ComfyEnv.FLAG_MODEL_PATHS in argv, "必须挂模型路径映射：$argv")
        assertTrue(ComfyEnv.FLAG_INPUT in argv, "必须对准共享 input（否则投放的参考图找不到）：$argv")
        assertTrue(ComfyEnv.FLAG_OUTPUT in argv, "必须对准共享 output（否则自动捕获收不到图）：$argv")
        val note = assertNotNull(plan.note)
        assertTrue(note.contains("ComfyHub 补的"), "要如实说明这是谁补的：$note")
    }

    @Test
    fun `模型把 extra_args 写成 JSON 字符串时也能解析并补全`() {
        val root = tmp()
        val env = ComfyEnv.desktop(desktopDir(root))

        // 现场形状（会话 0bedb3f5 的 17:54 那次调用就是这么写的，而 schema 声明的是数组）
        val args = buildJsonObject { put("extra_args", """["--port", "8189"]""") }
        val argv = ComfyEnv.prepareLaunch(args, env).args["extra_args"].asStrings()

        assertEquals(listOf("--port", "8189"), argv.take(2), "模型自己写的参数要留在最前面：$argv")
        assertTrue(ComfyEnv.FLAG_MODEL_PATHS in argv, "写错形状也该照样补全：$argv")
    }

    @Test
    fun `模型自己写过的参数一律不覆盖`() {
        val root = tmp()
        val env = assertNotNull(ComfyEnv.desktop(desktopDir(root)))

        val args = buildJsonObject {
            put("extra_args", buildJsonArray {
                add(JsonPrimitive(ComfyEnv.FLAG_MODEL_PATHS))
                add(JsonPrimitive("D:/my-own.yaml"))
                add(JsonPrimitive(ComfyEnv.FLAG_INPUT))
                add(JsonPrimitive("D:/my-input"))
            })
        }
        val plan = ComfyEnv.prepareLaunch(args, env)
        val argv = plan.args["extra_args"].asStrings()

        assertEquals(1, argv.count { it == ComfyEnv.FLAG_MODEL_PATHS }, "不该出现两次：$argv")
        assertEquals("D:/my-own.yaml", argv[1], "值必须还是模型给的那个：$argv")
        assertTrue("D:/my-input" in argv, "input 也不该被覆盖：$argv")
        assertTrue(env.inputDir.toString() !in argv, "不该再补我们的 input：$argv")
        assertTrue(assertNotNull(plan.note).contains("尊重模型自己写的"), "要说明没有覆盖：${plan.note}")
    }

    @Test
    fun `模型写了 base-directory 时不补 input output 但仍挂模型根`() {
        val root = tmp()
        val env = ComfyEnv.desktop(desktopDir(root))

        val args = buildJsonObject {
            put("extra_args", buildJsonArray {
                add(JsonPrimitive("--base-directory"))
                add(JsonPrimitive("D:/somewhere"))
            })
        }
        val argv = ComfyEnv.prepareLaunch(args, env).args["extra_args"].asStrings()

        assertTrue(ComfyEnv.FLAG_MODEL_PATHS in argv, "模型根还是要挂：$argv")
        assertTrue(ComfyEnv.FLAG_INPUT !in argv, "base-directory 说了算，别再插嘴：$argv")
        assertTrue(ComfyEnv.FLAG_OUTPUT !in argv, "同上：$argv")
    }

    @Test
    fun `目录不存在时不加那个参数`() {
        val root = tmp()
        val desk = desktopDir(root)
        // 把 output 删掉：settings.json 里还写着，但目录已经没了
        Files.delete(sharedOutput(root))

        val env = assertNotNull(ComfyEnv.desktop(desk))
        assertNull(env.outputDir, "不存在的目录不许给 ComfyUI（启动会直接失败）")

        val argv = ComfyEnv.prepareLaunch(buildJsonObject { }, env).args["extra_args"].asStrings()
        assertTrue(ComfyEnv.FLAG_OUTPUT !in argv, "不该把不存在的 output 传下去：$argv")
        assertTrue(ComfyEnv.FLAG_MODEL_PATHS in argv, "模型根照旧要挂：$argv")
    }

    @Test
    fun `拿不到 Desktop 配置时如实警告 不编参数`() {
        val args = buildJsonObject {
            put("extra_args", buildJsonArray { add(JsonPrimitive("--port")); add(JsonPrimitive("8189")) })
        }
        val plan = ComfyEnv.prepareLaunch(args, desktop = null)

        assertEquals(listOf("--port", "8189"), plan.args["extra_args"].asStrings(), "没有配置就一个参数都不许补")
        val note = assertNotNull(plan.note, "必须给出警告，不能静默")
        assertTrue(note.contains("看不到"), "要明说实例看不到共享模型：$note")
        assertTrue(note.contains("别据此断言"), "要挡住'本机没有某个模型'这种结论：$note")
    }

    @Test
    fun `extra_args 的三种形状都认`() {
        val asArray = buildJsonObject {
            put("extra_args", buildJsonArray { add(JsonPrimitive("--port")); add(JsonPrimitive("8189")) })
        }
        assertEquals(listOf("--port", "8189"), asArray["extra_args"].asArgvForTest())
        assertEquals(listOf("--port", "8189"), JsonPrimitive("""["--port","8189"]""").asArgvForTest())
        assertEquals(listOf("--port", "8189"), JsonPrimitive("--port 8189").asArgvForTest())
        assertTrue(JsonPrimitive("").asArgvForTest().isEmpty())
        assertTrue(null.asArgvForTest().isEmpty())
    }
}

/** 测试里只想看 argv 的内容。 */
private fun JsonElement?.asStrings(): List<String> =
    (this as? JsonArray)?.map { it.jsonPrimitive.content } ?: emptyList()

private fun JsonElement?.asArgvForTest(): List<String> = ComfyEnv.run { this@asArgvForTest.asArgv() }
