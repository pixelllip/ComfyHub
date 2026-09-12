# 反推提示词：冬日长椅上的蓝发少女（原图 720x950 → 推荐 768x1024）

> 由 anima-prompt（主线）+ anima-nsfw-prompt（本次判定为 **safe 档**，NSFW 解构不适用）产出。
> 判定依据：全年龄场景（公园长椅 + 雪 + 冬装），无露点、无色情氛围 → 安全档 `safe`，负面用 NEG_SAFE 基线按画面改写。

## 模板

**模板 C（叙事模板）**，用于成品级/默认质量；本次实际落成 **混合形态**：
`质量头 + 角色块（tag）→ 场景/镜头（tag）→ NL 正文（分号分层）→ 情绪收尾`。
理由：单角色 + 场景氛围主导，画面叙事性来自"冷 + 哈气取暖"而非 H 行为，走 C 比 B 的分段 tag-stack 更贴。

## 已识别并锁定的视觉锚点（逐条对应原图）

| 维度 | 锁定 tag |
| --- | --- |
| 发/眼 | `light blue hair, long hair, wavy hair, blunt bangs` / `grey eyes, round eyes, large pupils` |
| 头部 | `black beret`（黑贝雷帽，原图签名配件之一） |
| 服装 | `blue and white plaid scarf, thick scarf` + `light grey double-breasted coat` + `black sweater cuffs` + `dark grey pleated skirt` + `black thighhighs` |
| 动作（核心） | `sitting on a wooden park bench, leaning forward, hands clasped together in front of her mouth, both hands visible` |
| 签名细节 | `white breath, small puff of white breath`（原图最抓眼的叙事点：哈气） |
| 表情 | `looking down, gentle expression, not looking at viewer`（**不看镜头**，原图是低头垂眼） |
| 场景 | `empty winter park, snow-covered ground, bare trees, snow-covered bench slats, falling snow, footprints in the snow` |
| 光线 | `backlighting, rim light on her hair, slight overexposure, glowing white light and lens flare, cool white and pale blue tones`（逆光过曝 + 冷调，原图光感的关键） |
| 镜头 | `shallow depth of field, slightly tilted camera, from above, medium shot`（原图有轻微倾斜 + 俯视坐着的人） |

## 参数（实测）

- **模型**：本机只有 `anima-base-v1.0.safetensors`（无 aesthetic / turbo 权重）→ Base 档，质量词加 `score_9, score_8, score_7`。
- **LoRA**：`anima-aesthetic-improvement-v1.1.safetensors` @ 0.8。
- size **768x1024**（原图 720x950 ≈ 3:4，竖直构图保住长椅 + 雪地纵深）；sampler `er_sde`；scheduler `simple`；steps **30**；CFG **4.0**。
- 负面 69 词（NEG_SAFE 改写）：删掉 `looking at viewer`（原图不看镜头）、加 `chinese text / watermark / username`（压原图右下角水印）、压 `headphones, earmuffs, hat over ears`（防帽子被加耳罩/耳机）、压 `school uniform`（防外套被替换成制服）。
- seed：c1 = `771234`，c2 = `424242`。

## 出图

| 版本 | 文件 | 评价 |
| --- | --- | --- |
| c1 | `anima_snowgirl_c1_00001_.png` | 手捧哈气 + 呼气白雾最准，构图偏正 |
| c2 | `anima_snowgirl_c2_00001_.png` | 构图/倾斜角/逆光边缘光最接近原图，哈气弱一点（手改成半遮嘴） |

调参方向：想更接近原图"手捧成杯 + 明显哈气"用 **c1 的 action 行**；想更接近原图构图与光感用 **c2 的镜头/光线行**——两行拼起来再跑一次即可。

## 复跑

```powershell
pwsh -File scripts\anima-gen.ps1 -PromptFile .\.anima\snow_girl\positive_c2.txt `
  -NegativeFile .\.anima\snow_girl\negative.txt -Width 768 -Height 1024 -Steps 30 -Cfg 4.0 -Prefix anima_snowgirl_c3
```
