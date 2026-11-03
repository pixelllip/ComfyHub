# 2D 布局模板：工作台里的「提示词 + 可编辑构图」

> 状态：**已落地**（2026-11-03）。3D 路线同日搁置，见
> [`layout-3d-design.md`](layout-3d-design.md) 的头部说明。
> 画布本身与两条出口见 [`layout-translator.md`](layout-translator.md)；
> 本文只记**为什么换思路**、以及新链路的契约。

## 0. 用户口径与结论

原话（2026-11-03）：

> 经过两天的研究，我想布局设计模块应该换思路：用户在 AI 工作台提供描述、请求生成提示词，
> AI 生成提示词的同时给出可用的 2D 布局模板，我可打开预览并修改，修改后重新提交给工作台，
> 强调这是用户的实际用意。3D 路线就先弃用。

结论：**布局不是另一个工具页，而是提示词那一轮回复的一部分。** 工作台是主入口，
画布只是"把构图摆清楚"的那一步。3D（体块 / 相机 / 投影）全部不做。

## 1. 链路（一张图）

```
用户在工作台描述 → AI 写提示词（代码块）
                  └─ 同轮调 layout_propose（1~3 个构图草稿）
                        └─ 后端规范化 → tool_result.jsonPayload.layoutTemplates
                              └─ 回复末尾「2D 布局模板」卡（缩略图）
                                    └─ 打开画布（带 initialSpec）
                                          └─ 用户改框 →「提交回工作台」
                                                └─ 一条用户消息（散文 + 规格 JSON）
                                                      └─ AI 按新框重写提示词
```

关键点：**提交回工作台就是发一条普通用户消息**。没有新表、没有新端点、没有第二套
"布局会话"状态 —— 模型看到的与用户看到的都是同一条消息（用户气泡里由
`LayoutResubmitCard` 画成一张卡，点「再次修改」还能接着改这一版）。

## 2. 后端：`layout_propose`

真源：`server-rs/src/ai/layout_proposal.rs`（工具登记在
`tools/desktop_tools.rs`；`IMPLEMENTED`、`execute`、`ai_settings::category_of` 都从这里派生）。
系统提示：`nsfw_prompt.rs` **第 25 条**，`PROMPT_VERSION` bump 到 **v58**。

- **桌面专属**：手机没有画布，不进 run-kit 的 `tools[]`。
- **只读**（`ALLOW` / `mutating=false`）：不生成图片、不写库、不碰 ComfyUI，
  所以也不吃"联网 / ComfyUI 查询"那两份额度。
- 入参：`{ brief, templates[1..3] }`；每个模板 `{ title, note, width, height, camera, regions[1..8] }`。
- 出参（`jsonPayload`）：

```jsonc
{
  "layoutTemplates": [
    { "title": "三分法：旅人压左", "note": "…", "spec": { /* lib/models/layout_spec.dart 的 v1 形状 */ } }
  ],
  "issues": ["《三分法》的「教堂」越出画布，已夹回界内。"]
}
```

### 2.1 后端为什么必须校验

`LayoutSpec.fromJson` 刻意很宽容（认不出的字段回默认），因为布局会被用户手改、
被老版本写坏。宽容的代价是：越界框会画到画布外、`relationTo` 指向不存在的框、
`kind` 是个没见过的词。**校验只放在工具出结果这一处**（前端只负责画与改）：

| 情况 | 处理 | 为什么 |
| --- | --- | --- |
| `x/y` 越界、`w/h` 过大 | 夹回 0~1000（最小边 20） | 画出界的框在 MOSS JSON 里仍是合法数字，只在出图时显形 |
| 框没有 `label` 也没有 `caption` | 丢掉这个框 + `issues` | 散文只能写成"一处未命名的物体"，等于没画 |
| `relation` 指向不存在的框 / 自己 | 清成 `none` + `issues` | 宁可少一句关系，也不要"与「undefined」是同一个主体" |
| `kind` / `camera` / `surface` 不认识 | 回默认 + `issues` | 不能让一个脏词把整份规格打回 |
| 模板 / 框超量 | 只留前 3 个模板、前 8 个框 + `issues` | 一段散文稳定承载约 5 个约束，再多只是噪声 |
| 一个可用框都没有 | `INVALID_ARGUMENT` | 空卡片比没有卡片更让人困惑 |

`issues` 同时**喂回模型**（工具正文里逐条列出），所以下一次调用能自己改对；
但正文不重复贴坐标 —— 模型是这些坐标的作者，重复只会挤占上下文。

### 2.2 为什么 payload 要落进 `tool_result` 块

`executor::Sink::push_tool_result` 只放行白名单里的键（`mediaIds` / `asked` / …）。
布局规格**不像图片有 id 可以事后重新拉**：不落进块里，表现就是"当轮卡片在、
重开 App 之后整张卡消失"。所以那里显式放行 `layoutTemplates`，
回归是 `executor/tests.rs` 的「布局模板随工具结果落进块里_否则重开就没了」。

## 3. 前端

| 落点 | 作用 |
| --- | --- |
| `lib/models/layout_embed.dart` | `LayoutCandidate`（读 payload）+ `LayoutResubmit`（拼/读那条用户消息） |
| `lib/widgets/layout_thumbnail.dart` | 缩略图（`CustomPainter`，与画布共用 `layoutKindColor`） |
| `lib/widgets/layout_templates_card.dart` | 回复末尾的模板卡 + 用户气泡里的"已提交布局"卡 |
| `lib/pages/layout_canvas_page.dart` | 新增 `initialSpec` / `initialName` / `fromWorkspace` 与「提交回工作台」 |
| `lib/pages/ai_home_page.dart` | 卡片渲染、打开画布、把结果发回对话；输入区「+ → 布局画布…」是空白画布入口 |

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
| 工具执行接线 + payload 形状 + 正文不复述 JSON | `tests/api_test/tools.rs` 的 `layout_propose_returns_editable_templates_and_fixes_boxes` |
| 工具在桌面清单、不在手机 run-kit | `tests/api_test/tools.rs`、`ai_run_kit.rs`（清单逐项断言） |
| 规格随 `tool_result` 落库 | `executor/tests.rs` |
| 系统提示版本 | `run_kit/tests.rs` 的 v58 前缀 + `ai_run_kit.rs` |
| 候选解析 / 回传往返 / 320 宽不溢出 / 画布带入 | `test/layout_embed_test.dart` |
| 画布翻译仍是纯函数 | `test/layout_spec_test.dart`（未改） |

## 6. 已知取舍

1. **构图质量取决于模型**。坐标是模型写的粗稿，用户要改；`issues` 只保证"能画、能改"，
   不保证"好看"。真正的构图判断仍由人做。
2. **一次回复只留一张模板卡**（同一消息里多个 `layout_propose` 的结果按到达顺序合并去重）。
   模型重复调用不会出现一屏卡片，但也不会把"第二版"单独成卡。
3. **提交即发送**。画布里的「提交回工作台」直接发一条消息，不再让用户回到输入框确认；
   要求可以在提交对话框里改。
4. **3D 不做**。体块 / 相机 / 投影的收益（表面归属、遮挡、多画幅）暂时让位于
   "先把 2D 构图与提示词的闭环跑顺"。
