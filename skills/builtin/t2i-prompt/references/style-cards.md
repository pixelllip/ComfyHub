# t2i-prompt · 预置画风卡（参考件，2026-11-10）

三张卡从三张参照图归纳：**只锁"怎么画"**（媒介 / 线 / 上色 / 光 / 材质 / 背景纵深 / 后期），不含具体人物与剧情。卡正文可直接作为画风卡的 `prompt`，或在创作时作为 `[STYLE SPEC]` 逐字插入。

**使用纪律**

1. **逐字使用**：不要改写、不要压缩成 tag、不要和别家风格段混拼。
2. **卡管画风，主提示词管内容**：人物、动作、场景由主提示词写；卡只负责渲染规格。
3. **一张图只挂一张卡**；确需画师时最多 1 个，先出卡再同 seed 试画师。卡与画师冲突时以卡为准。
4. **配 B2 背景**：三张卡都依赖环境层次（前景 / 中景 / 背景 + 两个光源 + 空气层），配纯色背景会浪费卡。背景配方见每张卡末尾。
5. **质量头不进卡**：`masterpiece, best quality` 由主提示词给，卡里不重复。
6. **不自动入库**：这三份是 skill 参考件，不写入提示词库 / 画风卡库；用户要存成卡，由他在「预设库 → 画风卡 → 新建」里自己建。
7. 负面用卡里的，再按提示词引擎的「问题预测」追加；不要把所有负面清单无条件叠加。

---

## 卡 1 · 黑金哥特（Dark Gold Gothic）

**一眼识别**：深青护墙板 + 金框油画 + 黑缎 / 白蕾丝 + 冷蓝轮廓光 + 漆面高光。

**适合**：哥特萝莉、洋馆、暗色肖像、反派、吸血鬼 / 人偶题材。
**比例 / 镜头**：3:4 或 2:3；50-85mm，半身到全身。
**主色**：deep teal / near-black / ivory / antique gold；一点冷蓝 + 一点烛光暖。

**卡正文（prompt，逐字用）**

> The signature is lacquered darkness: near-black fabric with a wet, narrow specular, chalky ivory lace, deep teal-green walls and tarnished antique gold, lit by a cold blue rim against a small warm candle core.
>
> A high-detail Japanese anime illustration in a dark gothic key, built from crisp warm-gray ink lineart and soft semi-thick rendering. Linework holds every frill, ribbon, lace edge and corset seam, then dissolves where the cold rim light lands. Forms are modeled with smooth airbrushed gradients laid over hard cel shadows: near-black satin keeps a cool blue-violet shadow core, a deep matte body and one wet-looking highlight along each fold; ivory lace stays chalky and opaque, its pattern read through tiny punched shadows rather than white paint. Skin is rendered with a porcelain-smooth finish, a faint rose blush that fades before the jaw, a soft subsurface warmth at the ears and knuckles, and cool gray-violet contact shadows; eyes are large and glossy, each with one hard catchlight and a deep iris gradient. Hair falls in wide ribbon-like masses, one band highlight per lock that cools and dissolves at the tips, a few loose strands catching the rim light. Metal is antique — brushed gold, dull brass, tarnished chain — with broken soft highlights instead of mirror shine. Patent leather is the hardest surface in the frame: black, glassy, with a single long white reflection.
>
> Interiors are staged in real depth rather than painted flat: paneled walls and a row of gold-framed oil paintings sit one value darker than the figure, dark wooden furniture and an upholstered chair dissolve into soft bokeh, dust motes drift through the key beam, and a gentle vignette pulls the corners toward near-black. The finish is lacquered, nocturnal and slightly oppressive — high local contrast on the figure, low contrast and cool desaturation behind it, a faint blue bloom around the brightest highlights, and a fine film grain.

**negative**

> flat cel shading, pastel colors, bright daylight, white background, simple background, oversaturated, plastic skin, mirror-like gold, 3d, photorealistic, blurry, lowres, text, watermark, signature

**背景配方（B2）**

- **前景**：左下角虚化的紫色绒面椅背 / 深色桌角，只框边缘。
- **中景**：人物 + 一张深色边柜（黄铜烛台、干花小瓶）。
- **背景**：深青护墙板、两幅金框风景油画（一亮一暗）、左侧暗门洞。
- **光**：左上方冷蓝窗光作 key，中左侧暖烛光作 practical；发梢 / 肩线给冷色 rim。
- **空气**：光束里的尘埃、极淡蓝色微粒、四角暗角。

**禁忌**：别配大面积纯白 / 高调背景（会变成普通女仆图）；别加 `pastel`、`soft colors`；别用三画师串（背景会退成纯色）。
**可替换内容**：金框油画 / 护墙板可换成任何"深色墙面 + 成排金框 / 镜框 / 壁灯"的室内；保留明度压缩与冷蓝 × 暖烛光的对比即可。

**tag 速查（只做检索，不替代正文）**：`dark, gothic, high detail, chiaroscuro, rim light, gold frame, teal, black satin, lace, dust motes, vignette, painterly`

---

## 卡 2 · 琥珀暖木（Amber Wood）

**一眼识别**：琥珀棕整体色偏 + 蜂蜜黄 / 粉色 / 奶油 + 暖逆光 + 纸感颗粒 + 奶油散景。

**适合**：日常、治愈、咖啡店 / 小馆、怀旧、青梅竹马、生活感半身像。
**比例 / 镜头**：2:3 或 3:4；50mm，半身。
**主色**：toasted brown / honey yellow / dusty rose / cream / deep umber。

**卡正文（prompt，逐字用）**

> The signature is an amber-brown cast over everything: honey yellow, dusty rose and cream, shadows that stay warm umber instead of going black, thick warm-brown lineart, and a fine paper grain over the whole frame.
>
> A warm, nostalgic Japanese anime illustration in a soft cel style with a hand-painted finish. The lineart is warm brown and slightly uneven in weight, visibly sitting on top of the color rather than being painted over. Shading is cel-flat with soft airbrushed gradients at the transitions, never thick paint. The palette is moderate in saturation and narrow: toasted brown, honey yellow, dusty rose, cream and deep umber, with a sepia-amber cast as if lit by old tungsten bulbs.
>
> Light is soft and directional from the upper left: a warm key wraps the hair and the cheek, a thin amber rim separates the shoulders from the background, and shadows keep a rosy undertone. Skin is smooth and matte, with airbrushed blush across the cheeks and the bridge of the nose, a soft gradient at the eyelids and a small clean specular dot in each eye. Hair is drawn in broad soft strands with a single warm band highlight and slightly darker roots; braids read as chunky interlocking shapes, not individual hairs. Fabric is matte cotton and linen: a soft blouse folds in rounded creases with chalky highlights, a pinafore has a drier, papery surface, and lace trim is simplified into scalloped shapes with tiny punched holes. Buttons are dull brass with one soft highlight.
>
> Interiors are built from warm wood and shelves: varnished boards, rows of dark glass bottles and white-labeled jars suggested with simple vertical strokes and one soft highlight each, everything behind the subject dissolved into creamy brown bokeh. Atmosphere is intimate and slightly dusty — a faint glow around the lamp, gentle bloom on the brightest edges, low overall contrast, and a sun-faded, lived-in finish.

**negative**

> cold blue palette, neon colors, glossy digital painting, harsh contrast, sharp background, 3d, photorealistic, hyperdetailed, text, watermark, signature, oversaturated

**背景配方（B2）**

- **前景**：木桌边缘 / 椅背在画面下缘，轻微失焦。
- **中景**：人物 + 木质吧台（杯子、碟子）。
- **背景**：整面木货架，深色玻璃瓶 + 白标罐 + 一只粉色果酱罐，横板形成节奏。
- **光**：左上方暖吊灯作 key，右侧窗光作柔补；发顶 / 肩线给暖 rim。
- **空气**：杯口蒸汽、光柱里的尘埃、奶油色散景、纸张颗粒。

**禁忌**：别加 `cold`、`neon`、`high contrast`；别把背景写成纯色（这张卡的灵魂在货架与散景）；别配强蓝光。
**可替换内容**：货架 / 瓶子可换成任何"暖木 + 成排小物"的室内（书架、餐具柜、杂货铺）；保留横线节奏与奶油散景即可。

**tag 速查**：`warm, nostalgic, cel shading, amber, cozy, wooden shelves, bokeh, paper grain, 90s anime, soft light`

---

## 卡 3 · 雾河暮色（Misty River Dusk）

**一眼识别**：低饱和蓝灰 + 小面积灯笼暖橙 + 可见笔触 + 三层大气透视 + 电影感收尾。

**适合**：场景叙事、旅行、宽幅、开场 / 终章、世界观建立镜头。
**比例 / 镜头**：16:9 或 21:9；35mm wide shot，人物偏一侧、留出景物。
**主色**：slate blue-gray / faded olive / weathered wood / amber + rust。

**卡正文（prompt，逐字用）**

> The signature is desaturated blue-gray cut by small warm amber accents, visible brushwork instead of clean lineart, three clearly separated value planes, and a filmic, misty finish.
>
> A cinematic semi-realistic anime illustration painted in confident, visible brushstrokes. There is no clean vector lineart; edges are carved by value and color, with dry-brush texture and broken strokes along cloth, wood and water. The palette is restrained and atmospheric — slate blue-gray, faded olive, weathered wood brown and cold mist — broken only by amber lantern light and small rust-orange accents. Saturation is low and fairly even; the warmth is concentrated in a few small areas, not spread across the frame.
>
> Values are grouped into three planes: a dark near foreground kept almost in silhouette, a mid-tone figure carrying the highest local contrast, and a pale, low-contrast distance fading into mist. Light is narrative: one warm lantern acts as the key, casting a soft amber pool on the face, collar and the canopy fabric above, while cool dusk sky fills the shadows with blue; every background element is separated by which light touches it. Skin is painted with soft opaque strokes, warm in the lantern light and cool at the shadow edge, with minimal rendering and a few deliberate hard accents at the nose, lip and knuckles. Hair is a mass of broad, dry strokes with a warm rim on the lantern side and cool blue-gray in shadow, individual strands only at the silhouette. Fabric is heavy wool and worn leather, painted with broad scumbles, dry highlights and dark soft folds; metal is dull and scratched; the lantern glass holds a hot orange core.
>
> Water is the signature material: horizontal broken strokes, soft reflected amber columns, dark ripples with pale sky glints, and a hazy transition into the far bank. Distance is built with atmospheric perspective — trees and huts lose contrast and detail, mountains flatten into blue-gray shapes, mist sits in layers. The finish is filmic: subtle grain, gentle halation around the lantern, muted highlights, and no pure black or pure white.

**negative**

> clean vector lineart, flat cel shading, saturated colors, neon, harsh contrast, busy sharp background, 3d, photorealistic, oversharpened, text, watermark, signature

**背景配方（B3）**

- **前景**：船篷帆布 / 缆绳 / 近岸芦苇，压成剪影，框住画面上缘与下缘。
- **中景**：人物 + 悬挂灯笼（暖色锚点）+ 船板。
- **背景**：河面反光、对岸村舍暖窗、泊船、树影、三层远山。
- **光**：灯笼暖橙作 key，暮色冷蓝天光作 fill；水面拉出暖色倒影柱。
- **空气**：水面薄雾、远山雾层、灯笼光晕、胶片颗粒。

**禁忌**：别加 `clean lineart`、`flat color`、`high saturation`；别把背景写得太清楚（大气透视是这张卡的命）；别用近景特写（35mm 宽幅才成立）。
**可替换内容**：河 / 船可换成任何"前景框 + 中景主体 + 远景大气透视"的外景（湿街、运河、山谷、站台）；保留灯笼暖锚点与三层明度即可。

**tag 速查**：`cinematic, painterly, muted colors, atmospheric perspective, lantern light, mist, wide shot, brush strokes, low saturation`

---

## 三张卡的最小组合示例（人物与场景归主提示词）

下面是"主提示词骨架 + 卡正文"的接法演示：Tag 头只写人物与场景内容，卡正文整段接在后面，不重复卡里的材质 / 光 / 后期词。

**卡 1 · 黑金哥特（3:4，半身到全身）**

> masterpiece, best quality, safe, highres, newest, 1girl, gothic lolita, black maid dress, white lace trim, black corset with gold lacing, white ruffled petticoat, black ribbon hair bows, black maid headdress, long black hair, blunt bangs, pale skin, dark eyes, black thighhighs, garter straps; standing in a dark teal paneled salon, two gold-framed landscape paintings, a dark wooden sideboard, a purple upholstered chair, a brass candlestick; cold blue window light from the upper left, warm candle glow from the side; dust motes in the beam, out-of-focus chair back in the lower left; 50mm portrait, shallow depth of field. + 卡 1 正文

**卡 2 · 琥珀暖木（2:3，半身）**

> masterpiece, best quality, safe, highres, newest, 1girl, brown twin braids, white daisy hairpins, brown eyes, blush, open smile, yellow blouse, lace collar, pink pinafore dress, brass buttons; sitting at a wooden counter in a warm cafe, shelves crowded with dark glass bottles and white-labeled jars, a small pink jar of preserved fruit, a brass pendant lamp; warm amber key from the upper left, soft window fill; steam and dust in the light, creamy brown bokeh, paper grain; 50mm, shallow depth of field. + 卡 2 正文

**卡 3 · 雾河暮色（16:9，宽幅）**

> masterpiece, best quality, safe, highres, newest, 1girl, silver hair, messy long hair, small dark horns, pale skin, black high-collar coat, white shirt, orange armband, black skirt; standing on a wooden boat at dusk, a hanging lantern, a wide river, distant village huts with warm windows, moored boats, reeds, mist, layered mountains; warm lantern key, cool dusk fill; 35mm wide shot, atmospheric perspective, film grain. + 卡 3 正文

> 示例只演示结构，不限制人物。换角色时保留"人物块 → 环境块 → 光源 → 空气 → 镜头"的顺序，卡正文不动。
