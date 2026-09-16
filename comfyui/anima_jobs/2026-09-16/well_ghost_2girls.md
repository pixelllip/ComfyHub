# well_ghost_2girls · Anima 反推提示词

- 参考图：夜神社 · 石井 · 粉发少女 + 漂浮幽灵娘（双人 · 双蓝焰 · 满月）
- 模板：B（分段式，多角色 + 场景深度互动）
- 安全档：safe
- 画幅：896 x 1120（4:5，接近原图 976x1216）
- Checkpoint：miaomiaoHarem_anima16.safetensors
- Sampler：euler_ancestral / normal / steps 30 / cfg 4.5 / seed 20260916
- 输出前缀：2026-09-16/well_ghost_2girls

## 正向

```
masterpiece, best quality, safe, highres, newest, anime coloring
from front, full body, wide shot, candid composition, deep blue and violet night palette
2girls, original, @rurudo, 1girl sitting cross-legged on the edge of a stone well, short pink hair, large ahoge, green eyes, half-closed eyes, bored expression, hand on own cheek, looking to the side, japanese clothes, white sleeveless kimono with red trim, wide hanging sleeves, red ribbon ties, red skirt, bare thighs, white socks, red sandals, black hair ornament
1girl floating in the air behind her, long black hair with green streaks, red eyes, wide open grin, sharp teeth, fangs, ghost, japanese clothes, white kimono, green sash, blue fire floating in her palm, green hitodama above her head
floating blue flames, small blue spark drifting beside the sitting girl's head
night, full moon, torii gate, shimenawa rope, red paper lantern, stone well overgrown with ivy, distant city skyline with skyscrapers, mist, magenta rim light
both girls are occupied with their own business rather than posing for the camera; the living girl looks thoroughly unimpressed while the ghost celebrates behind her; evoking an atmosphere of quiet supernatural mischief
```

## 负向

```
worst quality, low quality, artist name, blurry, jpeg artifacts, bad anatomy, bad hands, missing fingers, extra digits, fewer digits, fused fingers, watermark, signature, text, 3d, realistic, extra limbs, mirror, reflection, duplicate, lowres, multiple views, comic, speech bubble, heterochromia, cloned face, extra girls, 3girls, extra character, day, daylight, bright background, washed out colors, standing on ground
```

## 反推要点（为什么这么写）

1. 双人同框是串染重灾区：`2girls` 锚 + 每角色一个连续段（坐在井上的活人 / 飘在身后的幽灵），负面常驻 `heterochromia, cloned face`。
2. 「漂浮」先验极强，容易画成站着：写 `floating in the air` + 负面 `standing on ground`，并用 `hitodama` / `blue fire floating in her palm` 交代离地参照。
3. 夜景不要只写「深夜」靠色温：显式给 `night, full moon, deep blue and violet night palette`，负面压 `day, daylight`。
4. 井沿藤蔓、鸟居注连绳、红灯笼各只给 1 个词，环境道具不堆砌（防抢角色注意力）。
5. 画师只给 1 位锁画风，可按需替换（这是风格锚，不是角色身份）。
