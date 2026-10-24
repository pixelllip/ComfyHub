---
name: card-writer
description: 写 / 改 / 优化角色卡与画风卡。每张卡只有**一版自然语言正面提示词**（`prompt`）：角色卡 600+ token、从头到脚写全每一个细节；画风卡 100~200 token，按"媒介与笔触 → 形体与上色 → 色调 → 光 → 阴影 → 高光/辉光 → 散景与颗粒 → 完成度"写。用户要 Danbooru tag 版时当场另写一版给他看，但不存进卡里。优化时不许改掉核心身份 / 画风。
whenToUse: 用户要建 / 改 / 优化角色卡或画风卡；用户给了一段自然语言描述或一串 Danbooru tag，想把它变成可复用的卡；用户说"这个角色卡不够详细 / 帮我补全 / 把 tag 转成描述 / 把描述转成 tag"；要复刻某种画风并把它存成可复用的卡时。
version: 2
---

# 角色卡 / 画风卡写作

这两种卡是**同等地位**的两类知识，不是主从关系：

| | 管什么 | 唯一一版正文的目标 |
| --- | --- | --- |
| **角色卡** | 画的是谁（身份 / 外观 / 服饰基线） | **600+ token**，从头到脚每一个细节 |
| **画风卡** | 画成什么样（画师 / 媒介 / 上色 / 光 / 质感） | **100~200 token**，一段话 |

一张卡的字段：

- `prompt`：**正面提示词**，整段**自然语言**。这是卡里唯一的正文（2026-10-24 起不再保留 Danbooru tag 版）；
- `negative`：负面提示词；
- `exampleMediaIds`：可选，把**真实存在**的画廊产物挂成示例（最多 24 条）。

正文为空 = 卡没写完，工具会提醒。**不要**把 tag 栈存进 `prompt`：一句话描述比 tag 栈
更耐改（换底模 / 换 tag 体系都不用重写）；要 tag 版时按用户要求**当场**翻一版给他看。

---

## 1. 先判断要做什么

1. **卡的种类**：角色卡（`save_character_card`）还是画风卡（`save_style_card`）。
   拿不准时看用户说的是"这个人 / 这个角色 / 这套衣服"还是"这种画风 / 这种上色 / 这个画师的感觉"。
2. **先查**：`search_character_cards` / `search_style_cards`（同名 = 同一张卡，合并保存）。
   - 已经有卡：把它读出来，看正文是否为空、哪里需要优化；
   - 没有卡：按下面的模板从零写。
3. **再补资料**：
   - 角色：`danbooru_lookup` 只能用来**核对写法 / 找同现 tag**（`shinku_(nte)` 而不是 `shinku` / 中文名），
     它给的是素材，不是正文；
   - 画风：可以用 `search_prompts` / `civitai_search` / `web_search` 找同类画风的写法，但**不要把别人的整段 prompt 原样抄成自己的卡**，要提炼成可复用的画风特征。
4. **示例产物**：用户给了产物 id / 刚生成了符合的图，才把 id 放进 `exampleMediaIds`；
   **没有真实 id 就不要编**，也不要用示例图代替文字描述。

---

## 2. 角色卡正面提示词（600+ token）

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

- 一段或两段连贯的英文 / 中文散文都可以；**不要写成 tag 堆**。
- 具体 > 抽象：不要写"很漂亮的眼睛"，写"外眼角微微上挑、虹膜是偏灰的猩红、下睫毛浓密"。
- 同一件事只写一次；不要在结尾重复堆"masterpiece, best quality"这类质量词（那是主提示词的事）。
- 不知道的细节**不要编核心身份**（例如用户只说"银发红眼"，不要擅自给她加"龙角"）；
  可以补通用细节（手指、鞋型、服装层次），但要在回复里说明哪些是你补的。

---

## 3. 画风卡正面提示词（100~200 token）

### 3.1 硬要求

- **100~200 token**，**一段话**。英文约 75~150 词，中文约 150~300 字。
- 只写**画风**：媒介、笔触、线条、形体、上色、色调、光线、阴影、高光、辉光、散景、颗粒、完成度。
- **不要写具体人物**（不要出现"一个女孩 / 红裙子 / 站在窗边"），也不要写场景叙事。
- 不要写成 tag 堆；不要只写"anime style, masterpiece"这种空话。

### 3.2 推荐句式（按顺序写）

1. **媒介与笔触**：`A digital anime illustration painted semi-thick:` / `A watercolor illustration:`；
2. **线条**：`thin warm-gray line work that dissolves where light lands`；
3. **形体与上色**：`forms modeled by airbrushed gradients rather than cel steps`；
4. **色调**：`a low-chroma blue-gray field cut by small islands of warmth`；
5. **阴影**：`shadows hue-shifted toward blue-violet and lifted off black`；
6. **光**：`tinted ice-white key, clipped-white bloom around hair strands and collar ridges`；
7. **接触阴影 / 环境色**：`cool gray-violet contact shadows`；
8. **散景 / 颗粒**：`creamy circular bokeh, faint film grain`；
9. **完成度**：`a luminous, transparent, high-key finish`。

### 3.3 对照样例（用户给的基准，长度约 130 token）

> A digital anime illustration painted semi-thick: thin warm-gray line work that dissolves where light lands, forms modeled by airbrushed gradients rather than cel steps, a low-chroma blue-gray field cut by small islands of warmth, shadows hue-shifted toward blue-violet and lifted off black, tinted ice-white key, clipped-white bloom around hair strands and collar ridges, cool gray-violet contact shadows, creamy circular bokeh, faint film grain, and a luminous, transparent, high-key finish.

写新画风时**保留这个信息密度**：每一句都要给出一个可执行的视觉决策，不要用形容词堆砌。

---

## 4. 用户要 Danbooru tag 版时（当场翻，不存卡）

- 这是**另一次输出**，不是卡里的字段：在回复里给一段逗号分隔的 tag 栈，并说明"这是按卡里的正文翻的，
  没有改动卡片"。
- 角色 tag 先 `danbooru_lookup` 核对写法（`shinku_(nte)`，不是 `shinku` / 中文名）；查不到就换近义词，
  **不要硬编**。画风 tag 从正文里的视觉决策一一映射，正文写了 bloom 才写 `bloom`。
- 兼容 tag 模型时，正文与 tag 版必须描述同一件事；**不要**因为生成了 tag 版就回头改写卡里的正文。

---

## 5. 优化流程

- 先问清楚优化方向：更详细 / 更像某个画师 / 更适合某个模型 / 修掉互相矛盾的描述。
- **保留核心身份 / 画风**：可以补细节、改顺序、换更准的词；不要换脸、换发色、换画风大类。
- 优化后仍然满足长度要求：角色 600+ token，画风 100~200 token。
- 改的是**整段正文**：给了新正文就整段替换（`save_*_card` 的合并语义是"没给的字段保留"，不是"逐句合并"）。

---

## 6. 保存前自检（逐条过）

- [ ] 正面提示词非空、是**一整段自然语言**（不是 tag 栈）？
- [ ] 角色 600+ token、覆盖第 2.2 节清单？画风 100~200 token、一段话？
- [ ] 角色卡没有写场景 / 画风 / 镜头？画风卡没有写具体人物？
- [ ] 描述内部没有互相矛盾的发色、服装、上色、光线？
- [ ] 示例产物 id 都是**真实存在**的？没有编 id？没有把示例图当成文字描述的替代品？
- [ ] 保存用的是合并保存，没给的字段（比如 `negative`）不会被清空？

## 7. 保存与回复

- 保存：`save_character_card` / `save_style_card`，字段名是 `prompt` / `negative` /
  `exampleMediaIds`；同名会合并，不会攒出第二张卡。
- 回复用户时说清楚：正文写了多长、覆盖了哪些部位 / 哪些画风维度、哪些细节是你补的、挂了几条示例。
  **不要**说"已经写得很详细了"却不给长度与覆盖范围。
- 用户要在界面上改 / 删 / 管理示例：告诉他去「预设库 → 卡片库」对应卡片上操作。
