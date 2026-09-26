package com.comfyhub

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * GraphParse 的回归测试。
 *
 * 两个 fixture 都不是编出来的：
 *  · classic  —— 教科书式的 KSampler 工作流
 *  · samplerChain —— 从本机真实产物（MiniMax H3 参考图生视频）的 PNG 里 dump 出来的结构：
 *    参数分散在 SamplerCustomAdvanced 的 sigmas / sampler / noise / guider 上，
 *    提示词在条件节点的 `prompt` 输入里，模型名在 DiffusionModelLoaderKJ 上。
 *
 * 之所以专门加这一组：早期实现只认「采样器自己身上有 steps/cfg」的图，
 * 遇到真实的自定义采样链就会得到一条「正向提示词为空、seed 为空」的记录 —— 捕获了等于没捕获。
 */
class GraphParseTest {

    private fun graph(json: String): JsonObject =
        Json.parseToJsonElement(json).let { it as JsonObject }

    @Test
    fun `经典 KSampler 工作流能解析出全部字段`() {
        val parsed = GraphParse.parse(
            graph(
                """
                {
                  "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}},
                  "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 1216, "height": 832, "batch_size": 4}},
                  "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "cyberpunk city street, heavy rain", "clip": ["4", 1]}},
                  "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "lowres, watermark", "clip": ["4", 1]}},
                  "8": {"class_type": "LoraLoader", "inputs": {"lora_name": "detail-tweaker.safetensors", "strength_model": 0.8, "strength_clip": 0.8, "model": ["4", 0], "clip": ["4", 1]}},
                  "3": {"class_type": "KSampler", "inputs": {"seed": 884213771, "steps": 32, "cfg": 7.5,
                        "sampler_name": "dpmpp_2m", "scheduler": "karras", "denoise": 1.0,
                        "model": ["8", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0]}},
                  "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "neon/neon-street", "images": ["3", 0]}}
                }
                """.trimIndent()
            )
        )

        assertEquals("cyberpunk city street, heavy rain", parsed.positive)
        assertEquals("lowres, watermark", parsed.negative)
        assertEquals("sd_xl_base_1.0.safetensors", parsed.checkpoint)
        assertEquals(1, parsed.loras.size)
        assertEquals("detail-tweaker.safetensors", parsed.loras[0].name)
        assertEquals(0.8, parsed.loras[0].weight)
        assertEquals("dpmpp_2m", parsed.sampler)
        assertEquals("karras", parsed.scheduler)
        assertEquals(32, parsed.steps)
        assertEquals(7.5, parsed.cfg)
        assertEquals(884213771L, parsed.seed)
        assertEquals(1216, parsed.width)
        assertEquals(832, parsed.height)
        assertEquals(4, parsed.batch)
        assertEquals("IMAGE", parsed.kind)
        assertEquals("neon-street", parsed.titleHint)
        assertEquals("1.0", parsed.extra["KSampler.denoise"])
    }

    @Test
    fun `自定义采样链（SamplerCustomAdvanced）也能解析出参数与提示词`() {
        val parsed = GraphParse.parse(
            graph(
                """
                {
                  "287": {"class_type": "SamplerCustomAdvanced",
                          "inputs": {"noise": ["414", 0], "guider": ["299", 0], "sampler": ["291", 0],
                                     "sigmas": ["288", 0], "latent_image": ["424", 1]}},
                  "288": {"class_type": "BasicScheduler",
                          "inputs": {"scheduler": "simple", "steps": 20, "denoise": 1.0, "model": ["412", 0]}},
                  "290": {"class_type": "ResolutionSelector",
                          "inputs": {"aspect_ratio": "16:9 (Widescreen)", "megapixels": 0.3, "multiple": 32}},
                  "291": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "res_multistep"}},
                  "297": {"class_type": "VAELoader", "inputs": {"vae_name": "minimax_h3_audio_vae_fp32.safetensors"}},
                  "298": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3vl_32b_h3.safetensors", "type": "minimax"}},
                  "299": {"class_type": "BasicGuider", "inputs": {"model": ["300", 0], "conditioning": ["424", 0]}},
                  "300": {"class_type": "LoraLoaderModelOnly",
                          "inputs": {"lora_name": "minimax_h3_ref2v_turbo_4step.safetensors", "strength_model": 0.75,
                                     "model": ["415", 0]}},
                  "412": {"class_type": "DiffusionModelLoaderKJ",
                          "inputs": {"model_name": "minimax_h3_ref2va_pruned_int8_convrot.safetensors"}},
                  "414": {"class_type": "RandomNoise", "inputs": {"noise_seed": 432879252998404}},
                  "415": {"class_type": "MiniMaxH3MemoryEfficientSageAttentionPatch", "inputs": {"model": ["412", 0]}},
                  "424": {"class_type": "MiniMaxH3ReferenceToVideo",
                          "inputs": {"prompt": "subject_definitions:\n<Subject 1> is the anime-style girl",
                                     "ref_image_size": "match", "width": ["290", 0], "height": ["290", 1],
                                     "clip": ["298", 0], "vae": ["306", 0]}},
                  "430": {"class_type": "VHS_VideoCombine",
                          "inputs": {"frame_rate": 24.0, "filename_prefix": "MiniMaxH3", "format": "video/h265-mp4",
                                     "save_output": true, "images": ["304", 0], "audio": ["302", 0]}}
                }
                """.trimIndent()
            )
        )

        assertTrue(parsed.positive.startsWith("subject_definitions"), "正向提示词应来自条件节点的 prompt 输入")
        assertNull(parsed.negative, "这类图没有负向提示词")
        assertEquals("res_multistep", parsed.sampler)
        assertEquals("simple", parsed.scheduler)
        assertEquals(20, parsed.steps)
        assertEquals(432879252998404L, parsed.seed)
        assertEquals("minimax_h3_ref2va_pruned_int8_convrot.safetensors", parsed.checkpoint)
        assertEquals(1, parsed.loras.size)
        assertEquals("minimax_h3_ref2v_turbo_4step.safetensors", parsed.loras[0].name)
        assertEquals(0.75, parsed.loras[0].weight)
        assertEquals("VIDEO", parsed.kind)
        assertEquals("MiniMaxH3", parsed.titleHint)
        assertEquals("16:9 (Widescreen)", parsed.extra["ResolutionSelector.aspect_ratio"])
    }

    @Test
    fun `空图或坏图不会抛异常`() {
        val empty = GraphParse.parse(null)
        assertEquals("", empty.positive)
        assertEquals("IMAGE", empty.kind)
        assertNull(empty.steps)

        val notANode = GraphParse.parse(graph("""{"3": "字符串不是节点"}"""))
        assertEquals("", notANode.positive)
    }

    @Test
    fun `没有正负向命名的图也能靠启发式认出文本`() {
        val parsed = GraphParse.parse(
            graph(
                """
                {
                  "1": {"class_type": "CLIPTextEncodeSDXL",
                        "inputs": {"text_g": "a very long positive prompt about a neon city at night",
                                   "text_l": "neon city", "clip": ["2", 0]}},
                  "2": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sdxl.safetensors"}},
                  "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 7.0,
                        "sampler_name": "euler", "scheduler": "normal", "model": ["2", 0],
                        "positive": ["1", 0], "latent_image": ["4", 0]}},
                  "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}}
                }
                """.trimIndent()
            )
        )
        assertEquals("a very long positive prompt about a neon city at night", parsed.positive)
        assertEquals(20, parsed.steps)
        assertEquals(1L, parsed.seed)
        assertEquals(1024, parsed.width)
    }
}
