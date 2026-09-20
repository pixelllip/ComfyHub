"""把 NsfwPrompt 两段拼回去的结果，机械写进测试的 SEGMENT_BEFORE_SPLIT 字面量。

不手工誊抄：从 NsfwPrompt.kt 里按声明名取出两段（trimIndent 语义），
拼成 PERSONA + "\\n" + NSFW，再写成 Kotlin raw string。这样测试断言的
"基准"与实现同源，任何对正文的改动都会立刻被 `SystemPromptNsfwTest` 抓住。
"""
import io
import os
import re

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = os.path.join(root, 'server/src/main/kotlin/com/comfyhub/ai/NsfwPrompt.kt')
test = os.path.join(root, 'server/src/test/kotlin/com/comfyhub/ai/tools/SystemPromptNsfwTest.kt')


def read(path):
    return io.open(path, encoding='utf-8').read().split('\n')


lines = read(src)


def block(name):
    d = next(i for i, l in enumerate(lines)
             if re.match(r'\s*val %s: String = """\s*$' % name, l))
    c = next(k for k in range(d + 1, len(lines)) if lines[k].strip() == '""".trimIndent()')
    body = [(l[8:] if l.startswith(' ' * 8) else l) for l in lines[d + 1:c]]
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    return body


persona = block('PERSONA_BLOCK')
nsfw = block('NSFW_BLOCK')
joined = persona + [''] + nsfw
print('PERSONA=%d NSFW=%d 拼接=%d' % (len(persona), len(nsfw), len(joined)))

tl = read(test)
d = next(i for i, l in enumerate(tl) if 'private val SEGMENT_BEFORE_SPLIT' in l)
o = next(i for i in range(d, len(tl)) if tl[i].rstrip().endswith('"""'))
c = next(i for i in range(o + 1, len(tl)) if tl[i].strip() == '""".trimIndent()')
body = [('            ' + l) if l else '' for l in joined]
out = tl[:o + 1] + body + tl[c:]
io.open(test, 'w', encoding='utf-8', newline='\n').write('\n'.join(out))
print('测试字面量已重写：%d 行 -> %d 行' % (c - o - 1, len(body)))
