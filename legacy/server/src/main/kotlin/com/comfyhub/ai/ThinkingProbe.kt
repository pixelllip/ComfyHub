package com.comfyhub.ai

import com.comfyhub.SettingsRepo
import java.time.Instant

/**
 * 思考档位的**实测校准**（2026-09-23"全做了"：把 [CapabilitySource.TESTED] 用起来）。
 *
 * ## 为什么需要它
 *
 * 模态与"是否支持思考"有数据集（models.dev）可查，但**档位阶梯的线上表达**没有标准接口 ——
 * 而且聚合站经常整体忽略思考参数（2026-09-20 实测）。唯一可靠的来源是
 * [AiUpstream.probeThinkingLadder] 真发极小请求看网关认不认。
 *
 * ## 三条纪律
 *
 *  1. **默认关 + 显式开关**：校准要花真金白银（虽然极小），所以 `ai.capabilities.probe`
 *     默认 off；只有设置页打开后「校准」按钮才会真的发请求；
 *  2. **manual 行绝不覆盖**：用户手填的档位是他的话，实测结果没有资格静默改掉；
 *     capability_source 也只在 `builtin` 行上更新为 `tested`（discovered 行的来源说的是
 *     模态出处，一行一个字段，不能因为校准了档位就把模态的"接口声明"抹掉）；
 *  3. **如实回报**：每次尝试的结果（含失败原因原文摘要）都回给界面，绝不美化。
 */
object ThinkingProbe {

    /** `app_settings` 的键：是否允许实测校准（发真实请求）。 */
    const val SETTING_KEY = "ai.capabilities.probe"

    /** 单个 Provider 一次最多校准多少个模型（防手滑全库烧钱；按界面顺序取前 N 个）。 */
    const val MAX_MODELS_PER_PROVIDER = 8

    fun enabled(): Boolean = SettingsRepo.get(SETTING_KEY) == "1"

    fun setEnabled(on: Boolean) = SettingsRepo.put(SETTING_KEY, if (on) "1" else "0")

    data class Outcome(
        val providerId: String,
        val modelId: String,
        val ok: Boolean,
        val acceptedTop: String? = null,
        val ladder: Map<String, String>? = null,
        val written: Boolean = false,
        val skipped: Boolean = false,
        val message: String,
    )

    /**
     * 校准一个模型并（在守卫通过时）写回。
     * 只动 `thinking_efforts` + `capability_verified_at`；`capability_source` 只在
     * builtin 行上提升为 tested（见纪律 ②）。
     */
    fun probeOne(provider: AiProviderDto, model: AiModelDto, secret: String?): Outcome {
        if (!model.reasoning) {
            return Outcome(provider.id, model.id, ok = false, skipped = true,
                message = "未声明支持推理，跳过")
        }
        if (model.capabilitySource == CapabilitySource.MANUAL.wire) {
            return Outcome(provider.id, model.id, ok = false, skipped = true,
                message = "手工声明（manual）不覆盖；要实测请先把该模型的来源改成非手工")
        }
        val outcome = AiUpstream.probeThinkingLadder(provider, model, secret)
        if (!outcome.ok || outcome.ladder == null) {
            return Outcome(provider.id, model.id, ok = outcome.ok, message = outcome.message)
        }
        // 写回：efforts 用校准结果；档位为空但模型仍声明推理是合法的（"不声明等级"）
        val now = Instant.now().toString()
        val current = AiRepo.getModel(provider.id, model.id)
            ?: return Outcome(provider.id, model.id, ok = false, message = "模型已不在库中")
        val nextSource = if (current.capabilitySource == CapabilitySource.BUILTIN.wire) {
            CapabilitySource.TESTED.wire
        } else {
            current.capabilitySource
        }
        AiRepo.updateThinkingCalibration(provider.id, model.id, outcome.ladder, nextSource, now)
        return Outcome(
            providerId = provider.id,
            modelId = model.id,
            ok = true,
            acceptedTop = outcome.acceptedTop,
            ladder = outcome.ladder,
            written = true,
            message = outcome.message,
        )
    }
}
