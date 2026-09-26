package com.comfyhub.ai.mcp

import com.comfyhub.ai.tools.ToolAccess
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * `McpToolPolicy` 的默认档是权限模型的一部分，回归盯着四条：
 *  1. **起停类默认 ask**（launch 2026-09-22、stop/restart 2026-09-23 用户拍板："权限跟随当前对话的
 *     权限设置，默认是 ask，我设置了 allow 那就 allow"）—— 尤其 stop/restart：实例起错了（没挂
 *     共享模型），模型必须能停下重拉，deny 会把它挡死在原地（会话 `7c1e7ecd` 的教训）；
 *  2. 更新 / 换版本 / 装节点 / 下模型仍然默认 **deny**：那是动用户的安装，不该由模型发起
 *     （deny 的工具不会下发给模型，见 `ToolRegistry.specs`）；
 *  3. 写盘 / 花钱的默认 **ask**，纯只读的默认 **allow**（这批查询正是接 comfy-mcp 的价值所在）；
 *  4. 设置页要显示的"为什么是这个档"每档都有一句人话。
 */
class McpToolPolicyTest {

    @Test
    fun `起停类默认 ask（launch 0922 与 stop_restart 0923 用户拍板）`() {
        for (name in listOf("launch_comfyui", "stop_comfyui", "restart_comfyui")) {
            assertEquals(ToolAccess.ASK, McpToolPolicy.accessFor(name), name)
        }
    }

    @Test
    fun `更新换版本装节点下模型仍然默认 deny`() {
        for (name in listOf(
            "update_comfyui",
            "switch_comfyui_version",
            "install_node",
            "download_model",
            "download",
        )) {
            assertEquals(ToolAccess.DENY, McpToolPolicy.accessFor(name), name)
        }
    }

    @Test
    fun `写盘花钱的默认 ask 只读的默认 allow`() {
        for (name in listOf("run_workflow", "run_template", "generate_image", "upload_file", "partner_generate")) {
            assertEquals(ToolAccess.ASK, McpToolPolicy.accessFor(name), name)
        }
        for (name in listOf("system_stats", "search_models", "get_vram_info")) {
            assertEquals(ToolAccess.ALLOW, McpToolPolicy.accessFor(name), name)
        }
    }

    @Test
    fun `每一档都有一句给设置页看的理由`() {
        for (name in listOf("launch_comfyui", "stop_comfyui", "run_workflow", "system_stats")) {
            assertTrue(McpToolPolicy.reasonFor(name).isNotBlank(), "reasonFor($name) 不应为空")
        }
    }
}
