# 2D 布局模板：工作台里的「提示词 + 可编辑构图」

> 状态：**已落地**（2026-11-03；2026-11-11 增加"写完提示词后强制生成一个构图核对模板"）。
> 3D 路线同日搁置，见 [`layout-3d-design.md`](layout-3d-design.md) 的头部说明。
> 画布本身与两条出口见 [`layout-translator.md`](layout-translator.md)；
> 本文只记**为什么换思路**、以及新链路的契约。

## 0. 用户口径与结论

原话（2026-11-03）：

> 经过两天的研究，我想布局设计模块应该换思路：用户在 AI 工作台提供描述、请求生成提示词，
> AI 生成提示词的同时给出可用的 2D 布局模板，我可打开预览并修改，修改后重新提交给工作台，
> 强调这是用户的实际用意。3D 路线就先弃用。

补充口径（2026-11-11）：

> 当AI生成提示词之后，强制它基于它写的提示词生成一个布局模板，以供后续生图结束之后
> 核对构图是否符合预期。该模板可以像之前的三模板结构一样，被用户重新编辑和提交。

结论：**布局不是另一个工具页，而是提示词那一轮回复的一部分。** 工作台是主入口，
画布只是"把构图摆清楚"的那一步。3D（体块 / 相机 / 投影）全部不做。

## 1. 链路（一张图）

```
用户在工作台描述 → AI 写提示词（代码块）
                  └─ 写完提示词之后调 layout_propose(purpose="verify",
                        prompt=<这版提示词原文>, templates=[恰好 1 个])
                        └─ 后端规范化 → tool_result.jsonPayload.layoutTemplates
                              └─ 回复末尾「构图核对模板」卡（缩略图 + 提示词摘录）
                                    ├─ 用户点开画布改框 →「提交回工作台」
                                    │     └─ 一条用户消息（散文 + 规格 JSON）
                                    │           └─ AI 按新框重写提示词 → 再生成新核对模板
                                    └─ 出图后拿模板和产物对照构图
```

出图与提交：AI 用 `comfy_submit(positivePrompt=…)` 提交**同一版提示词**时，后端要求
本 Run 里已经按这版提示词生成过核对模板，否则拒绝（`LAYOUT_TEMPLATE_REQUIRED` /
`LAYOUT_TEMPLATE_MISMATCH`）。模板因此一定和这次出图对得上。

关键点：**提交回工作台就是发一条普通用户消息**。没有新表、没有新端点、没有第二套
"布局会话"状态 —— 模型看到的与用户看到的都是同一条消息（用户气泡里由
`LayoutResubmitCard` 画成一张卡，点「再次修改」还能接着改这一版）。

## 2. 后端：`layout_propose`

真源：`server-rs/src/ai/layout_proposal.rs`（工具登记在
`tools/desktop_tools.rs`；`IMPLEMENTED`、`execute`、`ai_settings::category_of` 都从这里派生）。
系统提示：`nsfw_prompt.rs` **第 25 条**，`PROMPT_VERSION` 已 bump 到 **v70**。

- **桌面专属**：手机没有画布，不进 run-kit 的 `tools[]`。
- **只读**（`ALLOW` / `mutating=false`）：不生成图片、不写库、不碰 ComfyUI，
  所以也不吃"联网 / ComfyUI 查询"那两份额度。
- 两种用途共用同一份形状：
  - `purpose="verify"`（写完提示词后必调）：`prompt` 必填，`templates` 恰好 1 个；
  - `purpose="draft"`（用户额外要构图方案）：`templates` 1~3 个。
- 入参：`{ purpose, prompt, brief, templates[] }`；每个模板
  `{ title, note, width, height, camera, regions[1..8] }`。
- 出参（`jsonPayload`）：

```jsonc
{
  "purpose": "verify",
  "verificationPrompt": "<这版提示词原文>",
  "layoutTemplates": [
    {
      "title": "三分法：旅人压左",
      "note": "…",
      "purpose": "verify",
      "prompt": "<这版提示词原文>",
      "spec": { /* lib/models/layout_spec.dart 的 v1 形状 */ }
    }
  ],
  "issues": ["《三分法》的「教堂」越出画布，已夹回界内。"]
}
```

`purpose` / `prompt` 落在**每个模板**里，所以 `tool_result` 块（只放行
`layoutTemplates`）重开 App 之后仍然分得清"核对模板"与"构图草稿"。

### 2.1 后端为什么必须校验

`LayoutSpec.fromJson` 刻意很宽容（认不出的字段回默认），因为布局会被用户手改、
被老版本写坏。宽容的代价是：越界框会画到画布外、`relationTo` 指向不存在的框、
`kind` 是个没见过的词。**坐标与关系的校验只放在工具出结果这一处**（前端只负责画与改）；
"这版提示词有没有对应的核对模板"另有一道提交闸门（见 2.3）：

| 情况 | 处理 | 为什么 |
| --- | --- | --- |
| `x/y` 越界、`w/h` 过大 | 夹回 0~1000（最小边 20） | 画出界的框在 MOSS JSON 里仍是合法数字，只在出图时显形 |
| 框没有 `label` 也没有 `caption` | 丢掉这个框 + `issues` | 散文只能写成"一处未命名的物体"，等于没画 |
| `relation` 指向不存在的框 / 自己 | 清成 `none` + `issues` | 宁可少一句关系，也不要"与「undefined」是同一个主体" |
| `kind` / `camera` / `surface` 不认识 | 回默认 + `issues` | 不能让一个脏词把整份规格打回 |
| 模板 / 框超量 | 草稿只留前 3 个模板、前 8 个框 + `issues` | 一段散文稳定承载约 5 个约束，再多只是噪声 |
| `purpose=verify` 没给 `prompt` | `INVALID_ARGUMENT` | 没有提示词就无从核对"模板是否基于它" |
| `purpose=verify` 给了多个模板 | `INVALID_ARGUMENT` | 核对模板只有一个；要方案请走 `draft` |
| 一个可用框都没有 | `INVALID_ARGUMENT` | 空卡片比没有卡片更让人困惑 |

`issues` 同时**喂回模型**（工具正文里逐条列出），所以下一次调用能自己改对；
但正文不重复贴坐标 —— 模型是这些坐标的作者，重复只会挤占上下文。

### 2.2 为什么 payload 要落进 `tool_result` 块

`executor::Sink::push_tool_result` 只放行白名单里的键（`mediaIds` / `asked` / …）。
布局规格**不像图片有 id 可以事后重新拉**：不落进块里，表现就是"当轮卡片在、
重开 App 之后整张卡消失"。所以那里显式放行 `layoutTemplates`，
回归是 `executor/tests.rs` 的「布局模板随工具结果落进块里_否则重开就没了」。

### 2.3 提交闸门：同一版提示词必须已有核对模板

`comfy_submit(positivePrompt=…)` 进 `workflow_submit::tool_submit` 之前，`tools::execute`
先查本 Run 的 `ToolSession`：有没有一次成功的 `layout_propose(purpose="verify")`
依据过**这版提示词**。比对口径是"去掉首尾空白、连续空白压成一个空格"，只差空白算同一版。
没有 → `LAYOUT_TEMPLATE_REQUIRED`；有模板但对不上 → `LAYOUT_TEMPLATE_MISMATCH`。
两种失败都会把"下一步该怎么做"写进正文，模型据此重调 `layout_propose` 再提交。

只拦 `positivePrompt`：它是"这版提示词由模型定稿"的唯一结构化证据。照本地
`TextGenerate` 扩写、或只改 `overrides` 的提交不在这条闸门里（那些路径里没有一份
"模型写好的提示词原文"可以核对）。

### 2.4 模型自己也要看得到（否则这张卡只是给用户看的）

对照构图不能只发生在界面上。两条链路保证模型手里也有材料：

1. **模板**：`layout_propose(purpose="verify")` 的工具正文除了标题 / 框数，还会逐框列出
   **规范化之后**的坐标、类型、面与关系（例如
   `1. 提灯的旅人 [角色] x=40 y=300 w=260 h=640`）。模型自己写的参数可能已被夹边、改过
   id，所以不能只靠"它还记得自己写过什么"。
2. **产物**：`comfy_submit` 的结果里带 `referenceImages`（本次 `mediaIds` 对应的图片路径），
   执行器与 `comfy_view_outputs` 共用同一条附图链路：缩到 ≤1024 长边、按每结果 4 张 /
   每 Run 6 张的预算挂到**下一轮的 user 轮**上。模型因此能拿"刚生成的图"和"模板里的框"
   逐项对照位置 / 占比 / 遮挡 / 画幅。
3. **同一屏的核对要求**（2026-11-11）：`comfy_submit` 出图成功后，工具结果末尾会追加
   「出图后必须核对构图」+ 本次模板的框表（标题 / 画幅 / 每个框的坐标与关系）。规则写在
   几百行之外，产物图却在下一轮，模型很容易只回一句"已出图"；把核对对象放到工具结果里，
   它就没有"没看见要求"这一档。

边界与 `comfy_view_outputs` 完全一致：只有**声明了图片模态且协议支持 `inline_base64`**
的模型才会收到图；看不了图（纯文本模型 / 视频音频产物 / 附图预算用完）时，正文里没有
图片，模型必须如实说"我看不到这张图"，**不许**凭提示词编画面。第 25 条第 ⑧ 款把这条
写成了硬规则。

## 3. 前端

| 落点 | 作用 |
| --- | --- |
| `lib/models/layout_embed.dart` | `LayoutCandidate`（读 payload，含 `forVerification` / `prompt`）+ `LayoutResubmit`（拼/读那条用户消息） |
| `lib/widgets/layout_thumbnail.dart` | 缩略图（`CustomPainter`，与画布共用 `layoutKindColor`） |
| `lib/widgets/layout_templates_card.dart` | 回复末尾的模板卡 + 用户气泡里的"已提交布局"卡；核对模板显示标题「构图核对模板」与提示词摘录 |
| `lib/pages/layout_canvas_page.dart` | 新增 `initialSpec` / `initialName` / `fromWorkspace` 与「提交回工作台」 |
| `lib/pages/ai_home_page.dart` | 卡片渲染、打开画布、把结果发回对话；模板卡排在产物卡**之后**，先看生成的图再对照构图；输入区「+ → 布局画布…」是空白画布入口 |

### 3.1 回传消息的形状（**改了它历史卡片就认不出来**）

````
【2D 布局】<要求，默认"按这份布局重写提示词，构图以框为准。">

这份布局是我在画布上亲手定下的（不是建议稿），请按它重写提示词：…

<LayoutSpec.translate().prose>

以下是规格（坐标 0~1000，左上角为原点）：
```json
{"comfyhubLayout": { … spec … }}
```
````

`LayoutResubmit.tryParse` 只认三件事同时成立：开头标记 + **最后一个** ```json 围栏 +
`comfyhubLayout` 键。少一个都当普通文本（宁可少一张卡，也不误吃用户自己贴的 JSON）。

**为什么既给散文又给 JSON**：散文是纯函数投影，模型读不懂 JSON 时方位/比例还在；
JSON 是坐标无损的那一份，模型按框重写时用。

## 4. 边界（别让这张卡变成"出图"）

- 模板是**草稿**，不是产物：提示词正文里不许抄坐标，也不许说"已按布局生成"（第 25 条 ⑤）。
- 布局卡片不产生 `mediaIds`；真正的图仍然走 `comfy_submit`，产物入口只认后端的 `mediaIds`。
- 手机端没有画布，也没有这个工具（桌面专属）；手机上看到的仍是提示词本身。
- 不做"AI 自动应用布局"：改哪一版、改哪儿必须由用户点。

## 5. 验收与回归

| 防线 | 位置 |
| --- | --- |
| 规范化：夹边 / 丢无名称框 / 清坏关系 / 上限 / 空表报错 / 字符串化数组 / `front→inFront` | `layout_proposal.rs` 的 9 个单测 |
| `purpose=verify` 必须给提示词、只收 1 个模板、payload 带用途与提示词 | `layout_proposal.rs` 的 verify 单测 |
| 提交闸门：缺模板 / 提示词不一致 / 只差空白放行 | `layout_proposal.rs` 的提交闸门 3 例 + `tools.rs` 的执行接线 1 例 |
| 模型侧材料：verify 正文带规范化框表 | `layout_proposal.rs` 的 `verify_正文带规范化框表` |
| 出图后工具结果重复核对要求与框表（且只认本次提示词 / 不拿视频硬套） | `layout_proposal.rs` 的 `出图后核对提醒带框表且只认本次提示词` |
| 本次产物自动附图：`comfy_submit` 结果带 `referenceImages` | `tests/submit_test.rs` 的提交用例 |
| 只有我们自己的三条工具能触发附图 | `executor/tests.rs` 的 `只有我们自己产出的三条工具结果能把图附进下一轮` |
| 工具执行接线 + payload 形状 + 正文不复述 JSON | `tests/api_test/tools.rs` 的 `layout_propose_returns_editable_templates_and_fixes_boxes` |
| 工具在桌面清单、不在手机 run-kit | `tests/api_test/tools.rs`、`ai_run_kit.rs`（清单逐项断言） |
| 规格随 `tool_result` 落库 | `executor/tests.rs` |
| 系统提示版本 | `run_kit/tests.rs` 的 v70 前缀 + `ai_run_kit.rs` |
| 候选解析（含 verify）/ 回传往返 / 320 宽不溢出 / 画布带入 | `test/layout_embed_test.dart` |
| 画布翻译仍是纯函数 | `test/layout_spec_test.dart`（未改） |

## 6. 已知取舍

1. **构图质量取决于模型**。坐标是模型写的粗稿，用户要改；`issues` 只保证"能画、能改"，
   不保证"好看"。真正的构图判断仍由人做。
2. **一次回复只留一张模板卡**（同一消息里多个 `layout_propose` 的结果按到达顺序合并去重）。
   核对模板只有 1 个；用户额外要方案时草稿最多 3 个，合并后仍是同一张卡。
   模型重复调用不会出现一屏卡片，但也不会把"第二版"单独成卡。
3. **提交即发送**。画布里的「提交回工作台」直接发一条消息，不再让用户回到输入框确认；
   要求可以在提交对话框里改。
4. **3D 不做**。体块 / 相机 / 投影的收益（表面归属、遮挡、多画幅）暂时让位于
   "先把 2D 构图与提示词的闭环跑顺"。
