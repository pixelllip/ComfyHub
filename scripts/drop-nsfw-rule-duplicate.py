"""删掉 NSFW_BLOCK 里那份**陈旧的纪律重复**（用户 2026-09-26 拍板：纪律只留一份）。

背景：2026-09-20 把「越狱段」拆成 PERSONA_BLOCK + NSFW_BLOCK 时，规则 1~17 那一整份
被**又一次**抄进了 NSFW_BLOCK 的尾部。HarnessRunner 里那份重复当天删了，
但 NSFW_BLOCK 内部这一份留了下来 —— NSFW 开着时模型会读到两套规则，
而**排在后面的那份占上风**（它与新版口径相反：还写着 civitai 要先配令牌）。

这个脚本只做一件事：在 NSFW_BLOCK 内部，从那份重复的第一行
（`事实纪律（不要猜本机状态）：`，且必须是块内**第二次**出现）删到块尾，
把越狱段收在 `Behavioral rules` 那几行上（与 NsfwPrompt.kt 文件头声明的范围一致）。

用法：python scripts/drop-nsfw-rule-duplicate.py
"""
import io
import os
import re

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = os.path.join(root, 'server/src/main/kotlin/com/comfyhub/ai/NsfwPrompt.kt')

lines = io.open(src, encoding='utf-8').read().split('\n')

start = next(i for i, l in enumerate(lines) if re.match(r'\s*val NSFW_BLOCK: String = """\s*$', l))
end = next(i for i in range(start + 1, len(lines)) if lines[i].strip() == '""".trimIndent()')

# 块内第一次出现 `事实纪律` 是在那份重复里（越狱段本身没有这个词）——出现两次才是"有重复"
hits = [i for i in range(start + 1, end) if lines[i].strip() == '事实纪律（不要猜本机状态）：']
assert len(hits) == 1, '预期的重复段落不在了（hits=%d）— 先人工确认 NsfwPrompt.kt 现状' % len(hits)
cut = hits[0]
first_dup = lines[cut].strip()
assert first_dup == '事实纪律（不要猜本机状态）：', first_dup

removed = end - cut
# 保留块尾一个空行（与 PERSONA_BLOCK 的收尾一致）
kept = lines[:cut] + [''] + lines[end:]
assert kept[cut] == '' and kept[cut + 1].strip() == '""".trimIndent()'

io.open(src, 'w', encoding='utf-8', newline='\n').write('\n'.join(kept))
print('NSFW_BLOCK: 删掉 %d 行重复纪律（第 %d~%d 行）' % (removed, cut + 1, end))
