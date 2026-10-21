---
name: card-writer
description: 写 / 补 / 优化角色卡与画风卡。每张卡都要有 Danbooru tag 版（tagPrompt）和自然语言版（prompt）：角色卡的自然语言版 600+ token、从头到脚写全每一个细节；画风卡的自然语言版 100~200 token，按"媒介与笔触 → 形体与上色 → 色调 → 光 → 阴影 → 高光/辉光 → 散景与颗粒 → 完成度"写。只有一版时按已有那版补另一版；两版都有时按用户要求继续优化，但不许改掉核心身份 / 画风。
whenToUse: 用户要建 / 改 / 补 / 优化角色卡或画风卡；用户只给了 Danbooru tag 或只给了一段自然语言描述；用户说"这个角色卡不够详细 / 帮我补全 / 把 tag 转成描述 / 把描述转成 tag"；要复刻某种画风并把它存成可复用的卡时。
version: 1
---

# 角色卡 / 画风卡写作

这两种卡是**同等地位**的两类知识，不是主从关系：

| | 管什么 | 自然语言版目标 | tag 版目标 |
| --- | --- | --- | --- |
| **角色卡** | 画的是谁（身份 / 外观 / 服饰基线） | **600+ token**，从头到脚每一个细节 | 同一角色的规范 tag 栈（Danbooru 写法） |
| **画风卡** | 画成什么样（画师 / 媒介 / 上色 / 光 / 质感） | **100~200 token**，一段话 | 同一画风的 tag 栈 |

一张卡的两个正面字段：

- `tagPrompt`：**Danbooru tag 版**，逗号分隔，给 tag 模型直接照抄；
- `prompt`：**自然语言版**，整段文本，给自然语言模型 / 人读；
- `negative`：两版共用的负面词；
- `exampleMediaIds`：可选，把**真实存在**的画廊产物挂成示例（最多 24 条）。

只写一版是合法的，但**不算写完**：另一版必须补。两版都写完后才是一张完整的卡。

---

## 1. 先判断要做什么

1. **卡的种类**：角色卡（`save_character_card`）还是画风卡（`save_style_card`）。
   拿不准时看用户说的是"这个人 / 这个角色 / 这套衣服"还是"这种画风 / 这种上色 / 这个画师的感觉"。
2. **先查**：`search_character_cards` / `search_style_cards`（同名 = 同一张卡，合并保存）。
   - 已经有卡：把它读出来，看缺哪一版、哪一版需要优化；
   - 没有卡：按下面的模板从零写两版。
3. **再补资料**：
   - 角色：tag 写法必须先 `danbooru_lookup` 核对（`shinku_(nte)` 而不是 `shinku` / 中文名）；
   - 画风：可以用 `search_prompts` / `civitai_search` / `web_search` 找同类画风的写法，但**不要把别人的整段 prompt 原样抄成自己的卡**，要提炼成可复用的画风特征。
4. **示例产物**：用户给了产物 id / 刚生成了符合的图，才把 id 放进 `exampleMediaIds`；
   **没有真实 id 就不要编**，也不要用示例图代替文字描述。

---

## 2. 角色卡：自然语言版（600+ token）

### 2.1 硬要求

- **600+ token**。英文约 450~600 词，中文约 900~1200 字；写不到就说明细节还不够，继续补。
- **从头到脚**，不要只写"银发红眼的美少女"这种概括。
- 只写**角色本身**：外貌、身体、服饰、饰品、体态、惯用表情。
  **不要**写场景、背景、构图、镜头、画风、光影氛围（那些属于主提示词 / 画风卡）。
- 用**稳定的身份基线**：可以换姿势 / 换场景，但脸、发型、体型、服装层次不能每张图都变。
- 不写剧情、不写"她正在……"的动作叙事；要写成"她是什么样"的静态描述。

### 2.2 覆盖清单（逐项写，缺一项就问自己能不能补）

1. **整体**：年龄感、性别、体型（身高感 / 骨架 / 胖瘦 / 肌肉量）、肤色、整体气质。
2. **脸**：脸型、下颌线、颧骨、额头、眉毛形状与粗细、眼睛形状 / 大小 / 眼距 / 眼角、
   虹膜颜色与花纹、瞳孔、睫毛、双眼皮、卧蚕、鼻梁 / 鼻头 / 鼻翼、嘴唇厚度 / 唇形 / 唇色、
   牙齿、耳朵形状、耳垂。
3. **头发**：发色（主色 / 挑染 / 渐变 / 发根）、发长、发质（直 / 卷 / 蓬松 / 细软）、
   刘海、分缝、鬓发、发梢、常见扎法（马尾 / 双马尾 / 编发 / 发髻）、发饰。
4. **皮肤与标记**：肤色冷暖、质感、雀斑、痣、疤痕、纹身、胎记、晒痕、指甲。
5. **上半身**：颈、锁骨、肩宽、胸型、腰线、背、手臂粗细、手型、手指长度、指甲形状。
6. **下半身**：胯、臀、大腿、小腿、膝盖、脚踝、脚型、腿型。
7. **服装（分层写）**：内层、外层、领口、袖口、下摆、长度、材质、厚薄、图案、颜色、
   纽扣 / 拉链 / 绑带 / 蕾丝 / 褶皱、磨损与旧化。
8. **鞋袜**：鞋型、鞋跟、鞋底、颜色、材质、袜子长度 / 厚度 / 花纹。
9. **饰品与随身物**：耳饰、项链、戒指、手链、腰带、帽子、眼镜、发饰、包、武器 / 道具。
10. **体态与表情**：惯用站姿、肩颈习惯、手势、走路姿态、常见表情（笑 / 冷淡 / 害羞时的具体变化）。
11. **一致性钩子**：3~6 个"一眼认出是她"的锚点（例如异色瞳 + 左眉疤 + 红色发带），
    放在描述前部，方便后续出图复用。

### 2.3 写法

- 一段或两段连贯的英文 / 中文散文都可以；**不要写成 tag 堆**（tag 版是另一个字段）。
- 具体 > 抽象：不要写"很漂亮的眼睛"，写"外眼角微微上挑、虹膜是偏灰的猩红、下睫毛浓密"。
- 同一件事只写一次；不要在结尾重复堆"masterpiece, best quality"这类质量词（那是主提示词的事）。
- 不知道的细节**不要编核心身份**（例如用户只说"银发红眼"，不要擅自给她加"龙角"）；
  可以补通用细节（手指、鞋型、服装层次），但要在回复里说明哪些是你补的。

---

## 3. 角色卡：Danbooru tag 版

- 逗号分隔，**Danbooru 规范写法**；角色 tag 放最前，例如 `shinku_(nte)`。
- 建议 80~160 个 tag，按下面顺序分组（不写分组标题，只按顺序排）：
  1. 角色 / 作品：`shinku_(nte), nte_(series)`；
  2. 身份与体型：`1girl, solo, adult, tall, slender`；
  3. 脸与眼：`red_eyes, slit_pupils, long_eyelashes, sharp_eyes`；
  4. 头发：`silver_hair, long_hair, straight_hair, blunt_bangs, hair_ribbon`；
  5. 身体标记：`pale_skin, mole_under_eye, scar_on_cheek`；
  6. 服装（分层）：`school_uniform, white_shirt, red_neckerchief, pleated_skirt, black_thighhighs`；
  7. 鞋 / 饰品 / 道具：`loafers, hair_ornament, earrings`；
  8. 表情 / 体态：`serious, looking_at_viewer, hands_on_hips`；
  9. 必要的质量 / 构图 tag 只放少量，不要用它们凑长度。
- 拿不准的 tag 先 `danbooru_lookup`；查不到就换成能查到的近义词，**不要硬编**。
- 权重语法 `(tag:1.2)` 只在确实需要强调时用，不要整段加权。

---

## 4. 画风卡：自然语言版（100~200 token）

### 4.1 硬要求

- **100~200 token**，**一段话**。英文约 75~150 词，中文约 150~300 字。
- 只写**画风**：媒介、笔触、线条、形体、上色、色调、光线、阴影、高光、辉光、散景、颗粒、完成度。
- **不要写具体人物**（不要出现"一个女孩 / 红裙子 / 站在窗边"），也不要写场景叙事。
- 不要写成 tag 堆；不要只写"anime style, masterpiece"这种空话。

### 4.2 推荐句式（按顺序写）

1. **媒介与笔触**：`A digital anime illustration painted semi-thick:` / `A watercolor illustration:`；
2. **线条**：`thin warm-gray line work that dissolves where light lands`；
3. **形体与上色**：`forms modeled by airbrushed gradients rather than cel steps`；
4. **色调**：`a low-chroma blue-gray field cut by small islands of warmth`；
5. **阴影**：`shadows hue-shifted toward blue-violet and lifted off black`；
6. **光**：`tinted ice-white key, clipped-white bloom around hair strands and collar ridges`；
7. **接触阴影 / 环境色**：`cool gray-violet contact shadows`；
8. **散景 / 颗粒**：`creamy circular bokeh, faint film grain`；
9. **完成度**：`a luminous, transparent, high-key finish`。

### 4.3 对照样例（用户给的基准，长度约 130 token）

> A digital anime illustration painted semi-thick: thin warm-gray line work that dissolves where light lands, forms modeled by airbrushed gradients rather than cel steps, a low-chroma blue-gray field cut by small islands of warmth, shadows hue-shifted toward blue-violet and lifted off black, tinted ice-white key, clipped-white bloom around hair strands and collar ridges, cool gray-violet contact shadows, creamy circular bokeh, faint film grain, and a luminous, transparent, high-key finish.

写新画风时**保留这个信息密度**：每一句都要给出一个可执行的视觉决策，不要用形容词堆砌。

---

## 5. 画风卡：Danbooru tag 版

- 建议 20~50 个 tag，按下面顺序：
  1. 媒介 / 完成度：`digital_media_(artwork), anime_coloring, high_key`；
  2. 线条：`thin_lines, colored_lineart, warm_lineart`；
  3. 上色：`airbrushed, gradient_shading, no_lineart` 之类（按实际）；
  4. 色调：`limited_palette, blue_theme, low_saturation`；
  5. 光 / 阴影：`bloom, backlighting, colored_shadows, contact_shadow`；
  6. 质感：`film_grain, bokeh, depth_of_field`；
  7. 画师 / 风格参照：只在确实对应时写，例如 `artist_name`（不确定就不要写）。
- 画风 tag 与自然语言版必须**一一对得上**；自然语言里写了 bloom，tag 版就该有 `bloom`。

---

## 6. 补版 / 优化流程

### 6.1 只有一版 → 补另一版

- 只有 `tagPrompt`：先按 tag 还原角色 / 画风，再扩写成自然语言版。
  - 角色：把每个 tag 展开成可见细节，再补全清单里 tag 没覆盖的部分（手指、鞋、服装层次等）；
  - 画风：把 tag 按第 4.2 节的顺序组织成一段话，补上 tag 里没有明说的笔触 / 阴影 / 完成度。
- 只有 `prompt`：从自然语言里提炼规范 tag。
  - 角色：必须先 `danbooru_lookup` 核对角色 tag；查不到的属性用通用 tag；
  - 画风：把描述里的每个视觉决策映射成 tag，不要加描述里没有的风格词。
- 补完**两版一起保存**（`save_character_card` / `save_style_card` 一次带 `tagPrompt` + `prompt`），
  不要让另一版在合并时被空值覆盖。

### 6.2 两版都有 → 继续优化

- 先问清楚优化方向：更详细 / 更像某个画师 / 更适合某个模型 / 修掉互相矛盾的描述。
- **保留核心身份 / 画风**：可以补细节、改顺序、换更准的词；不要换脸、换发色、换画风大类。
- 优化后仍然满足长度要求：角色自然语言 600+ token，画风自然语言 100~200 token。
- 两版必须同步改：只改一版会造成"tag 版和自然语言版不是同一个东西"。

---

## 7. 保存前自检（逐条过）

- [ ] 两版都有？缺一版就先补，再保存。
- [ ] 角色自然语言版 **600+ token**、覆盖第 2.2 节清单？画风自然语言版 **100~200 token**、一段话？
- [ ] 角色卡没有写场景 / 画风 / 镜头？画风卡没有写具体人物？
- [ ] 两版描述的是同一个角色 / 同一种画风？有没有互相矛盾的发色、服装、上色、光线？
- [ ] tag 版是 Danbooru 规范写法？拿不准的查过 `danbooru_lookup`？
- [ ] 示例产物 id 都是**真实存在**的？没有编 id？没有把示例图当成文字描述的替代品？
- [ ] 保存用的是合并保存，没给的那一版不会被清空？

## 8. 保存与回复

- 保存：`save_character_card` / `save_style_card`，字段名是 `tagPrompt` / `prompt` / `negative` /
  `exampleMediaIds`；同名会合并，不会攒出第二张卡。
- 回复用户时说清楚：这张卡的两版各写了什么、缺的那版是怎么补的、哪些细节是你补的、
  挂了几条示例。**不要**说"已经写得很详细了"却不给长度与覆盖范围。
- 用户要在界面上改 / 删 / 管理示例：告诉他去「预设库 → 卡片库」对应卡片上操作。
