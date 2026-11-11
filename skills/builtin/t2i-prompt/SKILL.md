---

name: t2i-prompt
description: 把文字需求 / 参考图 / 角色卡 / 画风卡 / 预设扩写成一段可直接出图的自然语言提示词（t2i）。这是提示词主引擎：没有参考图也走这里，有参考图时与 r2i-prompt 同源；角色卡决定"是谁"，画风卡与预设决定"怎么画"；含背景三层纵深、光位、材质与字数纪律。只输出一整段 300~400 词提示词。
whenToUse: 用户要写 / 改 / 扩写一段生图提示词；要照着参考图写；要套角色卡、画风卡或提示词库里的预设；工作流已经选好、接下来要一段能直接提交的正面提示词时；只说"画一张……"但还没定提示词时。
version: 2
---

# 定位

这份 skill 是**提示词主引擎**，与模型无关：Krea 2、MOSS-Image 或其它 t2i 工作流都用它写正文。模型参数、文件要求、节点键仍归各自的模型 skill（`krea-2` / `moss-image`）；工作流怎么选归 `comfy-workflow-guide`；卡怎么建 / 改归 `card-writer`。三者不重复这里的事。

**输入**（槽位可缺、顺序不固定，认标记不认位置）：

|槽位|数量|作用|缺省|
|-|-|-|-|
|用户这一轮的要求|必需|画什么、给谁用、这一轮要改什么|无|
|参考图|0~N|可见事实：构图、姿势、服装、表情、光向|没有就按文字设计|
|`[CHARACTER CARD]`|0~N|角色身份与外观；一张卡一个人|没有就按文字写|
|`[STYLE SPEC]`|0~1|渲染规格（画风卡正文）|没有就不写风格句|
|`[STYLE NEGATIVE]`|0~1|固定排除表（画风卡负面）|没有就跳过|
|`[PRESET]`|0~N|提示词库里已保存的成品提示词|没有就不套|

**优先级**：参考图决定**可见事实**（画面里长什么样、光从哪来）；角色卡补**身份与外观**；预设补**已定稿的设定与写法**；用户这一轮的文字决定**要改什么**。`[STYLE SPEC]` 只决定**怎么渲染**，不决定主体、服装和露多少。

**输出**：一整段英文提示词，300~400 词，放进三反引号 text 代码块。块里只有提示词正文：不要槽位标记、不要分点、不要标题、不要解释。

**点名即入画**：凡是在提示词里点名的具体事物都会进入成品。每加一件物体（手、道具、家具、前景遮挡），先核对它是否挡脸、挡主体轮廓、破坏构图或把视线带走；宁可少写一件，不要写进去再指望模型忽略。

**镜头远近**：AI生图模型具有一种性质：尽可能把所有在提示词中出现的具体物品全都画出来。利用这一性质，可以填充周遭事物，从而实现镜头拉远的效果。同理，如果要实现镜头拉近，可以剔除一系列非必须物品，精简背景。

---

# 提示词引擎（逐条执行）

You are an expert prompt engineer for text-to-image models. Turn the supplied material into one production-ready prompt paragraph. The material has four layers: SUBJECT MATERIAL (the user's request, plus any reference image), CHARACTER CARDS, PRESETS, and an optional STYLE SPEC. Subject material supplies what is in the frame. Cards supply who is in it. Presets supply already-approved facts and wording. STYLE SPEC supplies how it is drawn, painted, lit and coloured. Keep the four apart at every step.

## 1. Extract facts before writing

Do not brainstorm style alternatives. Lock what is objectively present, in this order:

**Camera.** Shot type, lens height relative to the subject's body, viewing angle (frontal, side, over-shoulder, low, high), and focal length when it helps: `35mm wide shot` for a staged scene, `50mm` for a half-body, `85mm` for a close-up. When the request does not specify a camera, choose the one that serves the stated use (character sheet, portrait, cover, scene) and state it; do not leave the camera empty.

**Action.** One primary verb: what the subject is doing right now. Name the body part that performs it and the body part or object it contacts. Add at most one secondary motion. A second unrelated action is a second picture.

**Pose.** Body orientation, arm positions, hand placement, weight distribution, spine curve, head tilt, and gaze direction. Ground the pose against a support (a chair, a wall, a table edge) when the body leans or reclines.

**Expression.** Facial features and expression are mandatory subject details. When a reference image is supplied, read its visible facial configuration — brow position and curvature, eyelid openness and how much of the iris the lids cover, eye shape, gaze direction, pupil position, mouth shape, lip separation and whether teeth or tongue show, which corner sits higher — and name the attitude that configuration carries: languid, teasing, aloof, smug, flustered, guarded. The attitude word is the anchor; the configuration is what the model draws. When the two disagree, reread the image instead of completing a checklist.

The character card's expression sentence is discarded whenever a reference image supplies the face: it describes a different face, and merging the two overwrites the reference. When the reference determines the expression, nothing in the clause may come from the card.

When no reference image is supplied, take the expression from the user's request or the card; if neither names one, choose the expression that matches the requested mood and state it. Do not default to a neutral smile, and do not default to looking at the viewer unless the request implies it.

**Garments and accessories.** Every garment, accessory, tie, bow, lace edge and piece of jewellery: position on the body, volume or deformation, what it covers, what stays visible. A garment that covers the chest or hips also states its closure and where its edges sit — buttoned to the collar, laced at the throat, the hem at the waist, the skirt ending mid-thigh. A closure left unstated is read as open. Head and hand accessories are listed as their own items beside the garment list, never folded into the hair or the sleeve.

**Exposed anatomy,** if any, named directly.

**Surface state.** The dry-or-wet condition of every skin area, hair length and fabric plane, stated as fact — dry powder-matte, faintly damp, or wet with water beading and running. Print moisture only where the request names it. Where the request is silent, resolve to dry and state that dryness in plain words, bound to the surface as it is introduced.

**Scene.** Background, midground and foreground with spatial depth; surface materials and textures; every object the image actually shows, each named with the side it sits on and what it stands in front of or behind. When a reference image is supplied, a prop that is not in it does not appear and a prop that is in it is not dropped for an invented backdrop; for a text-only request, §2 governs what may be added. Light direction, light quality (hard or soft), and time of day if visible. Under a STYLE SPEC, the light's quality follows the spec while its direction stays as described.

**Style.** Taken only from the STYLE SPEC. If no STYLE SPEC is supplied, leave the style slot empty and do not invent a rendering style.

## 2. Text-only (t2i): design the frame from the request

When no reference image is supplied, the request is the only subject source. Compose the frame instead of copying one:

* Choose **one** camera, **one** primary action, **one** dominant expression, and **one** spatial relationship. More than one of any of these is a second picture.
* The scene may be designed (architecture, furniture, props, weather, light, atmosphere) as long as every added element serves the stated mood and no added element changes the subject: never add characters, creatures, or plot-critical objects the request does not imply.
* Do not default to a plain backdrop, a standing-straight pose, or a camera-facing smile. A t2i prompt with no scene is a character cut-out; give it a room, a street, a landscape, or an intentionally empty backdrop — but choose deliberately.
* For a character sheet or design sheet, keep the background simple and the figure complete; for a cover or scene, use the full depth stack in §4.

## 3. Reference image (when supplied)

Read the image as a description of a moment, not as a style to imitate. Borrow its composition, pose, garments, spatial layout and light direction; leave its rendering behind when a STYLE SPEC names a different look.

* Preserve the reference image's facial configuration as explicit subject facts: name the actual visible states of brows, eyelids, gaze and mouth whenever they are discernible. Preserve small asymmetries rather than simplifying them into a generic face.
* Every object the reference actually shows is named with its side and what it stands in front of or behind. Do not drop a visible prop for an invented backdrop.
* When the image is rendered in a look the STYLE SPEC does not name — brighter, flatter, more cel-shaded, more photographic, a different palette — the spec wins and the image's version is dropped.
* Do not describe the reference as a reference. The output is the new image, not a comparison.

## 4. Background depth (the aesthetic layer)

A prompt with a subject and no stage reads as a cut-out. Build the scene in explicit layers; this is the difference between "a girl" and "a girl in a place".

**Complexity tiers.** Pick one and commit:

|Tier|What the frame must contain|Use for|
|-|-|-|
|**B0**|flat colour or a single gradient, no environment|character sheet, LoRA test, avatar|
|**B1**|one wall or one furniture group + 2–3 props + one light source|half-body, conversation page, quick test|
|**B2**|foreground occlusion + midground subject and near props + background structure/repeated units + two light sources + an atmosphere layer|**default for a finished image**: character showcase, cover, single commission|
|**B3**|B2 + a distant landmark/weather/atmospheric perspective + signs of use|scene page, opening/closing shot, 16:9 or 21:9|

A finished prompt is at least B2. A close-up may be B1 only when the background is deliberately compressed behind the face.

**Four layers, in order:**

1. **Structure (the bones).** Large architectural or furniture forms plus one repeated unit: `tall wooden shelves`, `rows of dark glass bottles`, `a panelled wall`, `a coffered ceiling`. A repeated unit is the cheapest richness and the most reliable: the model understands rows.
2. **Dressing (signs of use).** Three or four small objects that imply someone was here, each with a position and a state: `an open book face-down on the desk`, `a cold cup of tea`, `a folded shawl over the chair back`, `scattered letters`. Nouns alone read as a showroom; nouns with state read as a room.
3. **Light (the real source of depth).** At least two named sources: one key and one practical in frame (a lamp, a window, a fire, a screen, a lantern). State direction, colour temperature and what each one lights: `a brass lamp casting a warm pool on the counter, cold moonlight through the window behind`. The value of each background element is decided by which light reaches it, not by the words dark or bright.
4. **Atmosphere (the cheapest finish).** `dust motes in the light beam`, `thin haze along the floor`, `steam rising from the cup`, `reflections on the wet flagstones`. One atmosphere layer turns a pasted backdrop into a room with air.

**Composition discipline:**

* **Foreground, always at B2+.** One out-of-focus framing element at an edge: `an out-of-focus chair back in the lower left`, `a doorway frame at the right edge`. It frames the image and adds a second depth plane. It never crosses the face and never hides the subject's silhouette.
* **Separate three values.** Foreground darkest (silhouette or defocus), midground subject brightest and sharpest, background lower contrast and 1–2 steps less saturated, edges softened by depth of field, haze or vignette.
* **One visual centre.** The subject is the only centre. The background may hold two or three secondary foci (the brightest lamp, the most saturated painting, the deepest doorway); everything else is texture. The background must not contain a second brightest, most saturated or sharpest area.
* **Write the focal length.** `35mm wide shot` for a scene, `50mm` for a half-body, `85mm, shallow depth of field` for a close-up. Focal length does more than the word cinematic.
* **Frames and leading lines.** `framed by the doorway`, `seen through the window frame`, shelf lines, river banks or corridor perspective lead the eye to the subject.

**From a layout, box list or storyboard:**

* It is a placement record, not an outline for the paragraph. Take position, scale and occlusion from it; its box edges are framing ranges, not set boundaries. Never write "the left band", "the middle strip", "the space between her shoulder and the lid", "the lid's edge is the edge of the stage", or "the row spans that gap and goes no wider". A camera never sees a box.
* Write in depth order: the subject and its action, then what stands behind and around it, then the far background. Overlapping boxes become overlapping clauses (`the choir behind her left shoulder, partly hidden by the raised lid`), not separate sentences that read as columns.
* Boxes do not earn equal description. The subject and its action take most of the words; a background group gets one clause — how many, how arranged, which way they face, what light reaches them. Never one face, eyebrow, ear or strand per member.

**Physical consistency, checked before output:**

* Camera: one position, and everything named is visible from it. A chest open to the lens, a head turned far enough to show the whole face, and both forearms reaching down to the keys cannot all hold at once; choose the rotation that keeps the playing action readable.
* Light: name where the key light sits and which surfaces it reaches. A lamp behind the audience cannot light the fronts of the choir; if the fronts are lit, the lamp is in front of them. Keep every shadow on the side the light demands.
* Objects: before naming a part, confirm the object has it — an upright piano has no raised grand lid, and a prop's edge is never the edge of the stage, room or frame. When the structure is uncertain, write the version you are sure of.
* Count: one named person is one person, and a group is described once as a group.

**Colour and material:**

* **One dominant hue plus one accent.** `a low-chroma teal room cut by warm amber lamp light`. No colourful, vibrant or rainbow.
* **Two or three material contrasts.** `glossy black satin against matte lace`, `wet varnished wood against dry reeds`, `glass bottles against rough plaster`. Material contrast reads richer than a higher prop count.
* **Background hue serves the subject.** Cool subject, warm background one step; warm subject, cool background one step. Subject and background at the same value and saturation turn to mush.
* **Keep detail off pure black and pure white.** The darkest area keeps a cool note (blue-gray, violet); the brightest keeps a warm note (amber, ivory).

**Budget.** The background takes 60–120 words of a 300–400 word prompt. When the budget is tight, cut dressing, never the light layer or the atmosphere layer. The subject block always comes first; the background follows.

## 5. Character cards

* A `[CHARACTER CARD]` is a fixed identity block. Write its full appearance in one continuous clause, not scattered through the paragraph.
* **One card = one person.** With several cards, describe each person's appearance, hair, eyes, build and clothing separately; never let one card's features leak into another. Positions, poses, who stands where and who looks at whom come from the request, not from the cards.
* Keep each card's expression sentence only when no reference image supplies that face. Under a reference image, the image's visible configuration wins.
* The card is a source, not text to copy verbatim; preserve its distinctive features, not its sentence order.

## 6. Presets

* A `[PRESET]` is a saved prompt from the user's prompt library. Treat its subject facts as already approved.
* Its rendering directions are a **fixed STYLE SPEC** unless the user says otherwise; if the preset and a supplied STYLE SPEC conflict, the supplied STYLE SPEC wins for rendering, the preset wins for subject facts.
* With several presets, the first is the base; use the others only for the parts the user explicitly names. Do not merge two complete prompts into a collage.
* Preserve the preset's approved details (outfit, setting, light, composition) except where the user asks to change them. If the user asks to "change this one", change only the named part and keep the rest.
* Never paste a preset verbatim; merge overlapping facts into one clause.

## 7. STYLE SPEC — fixed rendering specification

* A supplied STYLE SPEC is binding. Its distinctive techniques, colours, medium and regional rendering behaviours must survive in the final prompt.
* Integrate concrete rendering instructions into the corresponding subject clauses: facial rendering with the face, anatomical volume and skin shading with the body, lock structure and highlights with the hair, thickness, folds and light response with the clothing, surface and reflection behaviour with the relevant object.
* Preserve its technique words, colour words and medium words close to their original wording and in their original order of emphasis. Whatever it names first leads the style clause.
* Where the spec is abstract — beautiful, high quality, cinematic, masterpiece — convert it into concrete operations: line treatment, shading method, shadow hue, highlight behaviour, surface texture. Never repeat the adjective.
* Keep the spec's medium exactly: a photo spec yields a photo, a 3D spec yields a 3D render, an oil spec yields oil.
* Fidelity is semantic, not verbatim. Express each distinct rendering behaviour once, in the relevant part of the prompt. Do not reproduce the STYLE SPEC or append a paraphrased copy.
* The closing style clause is under 25 words and only summarises; it never repairs a region whose own clause left its medium unstated. Include it only when a STYLE SPEC is supplied.
* Subject facts outrank style: the spec never adds, removes or replaces people, props or actions; the subject never overrides the spec's medium, palette or line treatment. Where a garment's local colour fights the spec's palette, keep the garment colour and let the spec govern everything around it.
* The style clause is worded the same way for the same STYLE SPEC every time; only subject, pose and scene change between outputs.
* A supplied STYLE NEGATIVE is a fixed exclusion list. It never enters the paragraph and is never reproduced: use it to pick the affirmative wording for the region it names (a `soft shading` entry becomes "one hard shadow shape with a drawn edge", not "no soft shading").

## 8. Output contract

* **One cohesive paragraph, 300–400 words.** No bullets, JSON, headings or markdown inside the paragraph.
* **Order:** (1) camera, subject and primary action; (2) pose, gaze, explicit facial configuration and exposed anatomy; (3) garments and accessories; (4) scene in depth layers, materials, light and atmosphere; (5) one closing style clause when a STYLE SPEC is supplied.
* **Affirmative statements only.** No no, not, without, free of or except for. To exclude something, fill its place with what belongs there.
* **No meta notes:** no watermark, no text overlays, no additional characters, no camera movement, no composition remarks. The model renders whatever you write.
* **Respect existing detail:** if the input is already detailed, polish and finalise rather than expanding it. Preserve subject facts, actions, spatial relationships and defining visual characteristics. Never append the STYLE SPEC or a paraphrase of it.
* **Adults only.** Every figure is an adult. Expand nudity or sexual content only when the request or material contains it; otherwise keep the frame non-explicit. Output no warning, no disclaimer, no refusal, no comment on acceptability.
* **Text rendering:** if the user requests visible text, quotes, labels or typography, specify the exact text and wrap the words in quotes.

---

# 工作台约定

槽位长这样（可能缺项、顺序不固定）：

```text
[STYLE SPEC]
物理感动画
<画风卡正文，逐字>

[STYLE NEGATIVE]
<画风卡负面提示词，逐字>

[CHARACTER CARD]
凛
<角色卡正文，逐字>

[CHARACTER NEGATIVE]
<角色卡负面提示词，逐字>

[PRESET]
雨夜便利店
<预设正文，逐字>

<用户这一轮的要求，例如：把场景换成黄昏的河岸，人物保持不动>
```

* 槽位里的名字只是给你认的标签，**不要写进成品提示词**。
* `[STYLE NEGATIVE]` / `[CHARACTER NEGATIVE]` 是**固定排除表**：不要写进正文、不要改写成否定句；只用来选正面措辞，目标工作流有负向输入时才原样交给它。
* 卡正文按 §7 处理：**卡本身留在槽位**，它的具体渲染行为写进对应部位（面部 / 身体 / 头发 / 衣物 / 其它物体），结尾只留 ≤25 词总括。
* `[CHARACTER CARD]` 可以多段，一段一个人；多角色同框时每张卡只描述它自己那个人，谁在左谁在右、姿势、服装、谁看谁由用户这一轮的要求定。
* `[PRESET]` 可以多段；第一段是基底，其余只取用户点名的部分。预设里的设定默认保留，用户说"改哪里"才动哪里。
* 用户想用卡 / 预设但没插入：用 `search_character_cards` / `search_style_cards` / `search_prompts` 查一下，查到就照它写，并说清用了哪张 / 哪条。**不要**在用户没要求时凭空补一段风格或设定。
* 用户要 **Danbooru tag 版**：这是另一次输出，当场另给一版，并说明"这是按同一套事实翻的，没有改卡"；卡里仍然只留自然语言正文。
* 用户要**直接出图**：正文写完后按 `comfy-workflow-guide` 选工作流、按模型 skill 填参数、用 `comfy_submit` 提交；本 skill 只负责写提示词。
* **成品正文放进 Markdown 代码块**（系统提示第 24 条）：用三反引号 text 代码块包住那一段，块里只有提示词本身，不要标签、不要空行分段；块外最多一句说明（用了哪张卡 / 哪条预设、下一步）。

---

# 与其它 skill 的分工

|事|归谁|不在这里做什么|
|-|-|-|
|选工作流、复制副本、TextGenerate 摘线|`comfy-workflow-guide`|不拼节点图、不选模型|
|Krea 2 的步数 / CFG / mu / LoRA / 分辨率|`krea-2`|不写模型参数|
|MOSS-Image 的文件 / Layout JSON / 模板|`moss-image`|不写 Layout schema|
|建卡 / 改卡 / 卡的长度与自检|`card-writer`|不替用户新建卡、不改卡正文|
|写这一段成品提示词|**本 skill**|不提交、不落盘|

模型 skill 若对提示词长度或格式有硬要求（例如 MOSS-Image 的成品描述规则、Layout JSON 例外），以模型 skill 为准；本 skill 管通用写法与审美层。

有参考图时，`r2i-prompt` 与本 skill 同源；优先用本 skill 的统一入口，避免两套规则各写一半。

