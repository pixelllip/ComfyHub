# AI 工具与 Skills（M4 / M5）设计与实现说明

> 2026-09-16 起实施。对应需求：AIH-021/022/024（事件协议与取消）、AIH-033~036（Comfy 工具与轮数限制）、
> AIH-037~045（Skills 与安全）、AIH-046/047（提示词与注入防护）、AIH-049（工具调用状态卡）、AIK-002（效仿 DSH）。
>
> 配套调查记录：[`dsh-tool-layer-report.md`](dsh-tool-layer-report.md)（对 DeepSeek Harness 工具层的逐项实测）。

## 0. 一句话

**AI 工作台现在真的会调工具了**：模型流里出现工具调用 → 后端按权限策略执行 → 结果作为 `tool` 轮喂回去 →
直到模型给出正文或到达轮数上限；Skills 落在磁盘上，AI 说一句「记住这个流程」就能注册，用户在右侧栏能删，
**不需要重启应用**。

## 1. DSH 的工具里，我们抄了哪些、不抄哪些

DSH 暴露给模型的东西分五类。对照结论（`dsh-tool-layer-report.md` §1）：

| DSH 工具 | 我们是否采纳 | 说明 |
| --- | --- | --- |
| `read` / `write` / `edit` / `glob` / `grep` | **部分采纳** | 采纳"文件工具"这一类，但**收敛成 4 个**：`read_file` / `list_dir` / `write_file`（+ skills 专用写工具）。没有 `edit`（字面量替换）与 `glob`/`grep`（需要 ripgrep 二进制）——首期用不上，等真的有"改 ComfyUI 配置"这类需求再加 |
| `read_image` | 不采纳 | 我们的多模态入口是附件事务（M3），不是工具；工具里塞图片会把 token 账算乱 |
| `pwsh` / `bash` | **明确不采纳** | AIH-045 与实施方案 §2.3 都排除了 shell；"能跑命令"等于把权限模型绕过去了 |
| `subagent` / `subagent_fork` / `send_message` / `interrupt_agent` / `list_agents` | 不采纳 | 我们是单模型对话产品，不是多智能体编排器 |
| `workflow` / `ralph` | 不采纳 | 同上 |
| `job_output` / `job_list` / `job_kill` | 不采纳 | 没有长任务后台作业的概念（ComfyUI 那边是轮询捕获，不是 job） |
| `todo_write` / `ask_user_question` | 暂不采纳 | 属于交互协议，等有了"多步任务面板"再说 |
| `web_search` / `web_fetch` | 不采纳（首期） | 需要外部搜索服务与密钥，且与"本地创作工具"定位不符 |
| `present` / `exit_plan_mode` / `goal` 系列 | 不采纳 | 交付物在画廊里，不在工具里 |
| **`skill`（加载 Skill 正文）** | **采纳** | 对应我们的 `load_skill`，并补了 `list_skills` / `register_skill` / `delete_skill` |
| Skills 目录注入 | **采纳思路，实现不同** | DSH 把目录作为 **user 消息**（`<system-reminder>` + `<available_skills>`）注入，为的是不打断系统前缀的 KV cache；我们的系统提示本来就带时间戳（每 Run 变化），所以直接写进系统提示的独立区块，省一层消息 |

我们的工具清单（出厂 11 个，全部可在设置里改成 `allow / ask / deny`）：

| 工具 | 类别 | 权限档 | 说明 |
| --- | --- | --- | --- |
| `list_skills` | skill | allow | 列出本机 Skills（名称/描述/是否合法） |
| `load_skill` | skill | allow | 取完整正文，包成 `<skill_content>`；同一 Run 不重复加载 |
| `register_skill` | skill | allow | **AI 说一声就把 Skill 写到本机**（用户核心诉求） |
| `delete_skill` | skill | **ask** | 不可逆，默认要用户点批准 |
| `remember` | memory | allow | 把一条跨对话成立的事实写进长期记忆（M6）；写的是应用自己的 `memory.md`，不是用户文件系统 |
| `list_dir` | files | allow | 列目录（默认 ComfyUI 目录，深度 ≤2） |
| `read_file` | files | allow | 读文本，上限 256KB |
| `write_file` | files | allow | **只能写 ComfyUI 目录**（越界直接拒绝） |
| `comfy_get_status` | comfy | allow | 复用 `ComfyCapture.status()`：连通性 / 队列 / 最近捕获 |
| `comfy_get_run` | comfy | allow | 按 runKey 查一次捕获（AIH-034） |
| `comfy_sync_history` | comfy | **ask** | 会写我们的库，按 DEC-005 默认要审批 |

## 2. 权限模型：为什么不是照搬 DSH 的 preset

DSH 用 `read-only / workspace-write / danger-full-access` 三档 preset + `workspaceRoot` 表达权限
（`dsh-tool-layer-report.md` §4：**没有** `autoApprove` / `allowedDirectories` 这类白名单）。
那是给"AI 帮我在这个仓库里干活"设计的：整个工作区都该可写。

我们面对的是**内容创作者**，不是开发者；他们机器上这个目录里有 `.mysql`（数据库）、`.run`（进程状态）、
`storage`（作品）、`server`（后端）。写坏一个的代价是不对称的：AI 多写一个文件没什么价值，
但把数据库目录或版本库弄坏，用户就得重装。所以我们的默认是：

```
写：只允许 <项目根>\comfyui            ← 用户要求的"默认不能修改 comfy 目录以外的内容"
读：<项目根>\comfyui、<项目根>\storage  ← 产物目录只读
永远不可写：.git / .mysql / .run / node_modules（即使用户把白名单放宽到项目根，也仍然拒绝）
```

三条补充规则：

1. **路径判定走真实路径**：`..`、绝对路径、符号链接、短路径名都不能绕过（`ToolPolicy.realPath()`：
   存在就用 `toRealPath()`，不存在就取最近的存在祖先再拼回剩余部分）。
2. **逐工具三态**：`allow` 直接执行、`ask` 弹工具卡等用户点批准、`deny` **根本不下发给模型**
   （与其让模型看见再被骗着调用，不如不让它知道）。用户覆盖记在 `app_settings` 的 `ai.tools.policy`。
3. **审批超时 = 拒绝**：默认 5 分钟没人点、或 Run 被取消，都按拒绝处理 —— 绝不允许"没人管就默认执行"。

> 移植自 DSH 的两个细节：工具的 JSON Schema 里 `additionalProperties: false`（模型乱加参数会当场报错，
> 而不是悄悄忽略）；工具结果**视为不可信数据**，写进系统提示第 4 条（防提示注入，对应 RSK-004）。

## 3. 工具循环（HarnessRunner）

```
系统提示 + 历史（含历史里的 tool_call/tool_result）
  └─ while (轮数 < maxToolSteps=8)
       ├─ 请求上游（带 tools 定义）
       ├─ 收流：text / reasoning / tool_call 分片 → ToolCallAccumulator 拼完整
       ├─ 没有工具调用 → 结束（这就是最终回复）
       └─ 有工具调用 → 逐个：策略 →（必要时审批）→ 执行 → 落 ai_tool_calls
                      → 事件 tool.requested/started/completed/failed
                      → 结果作为 role=tool 轮喂回去
  └─ 最后一轮（step == maxSteps）**不带 tools**：逼模型用正文收尾，而不是继续要工具
```

要点：

- **一条助手消息装下整轮**：正文 / 思考 / 工具调用 / 工具结果都按 `ordinal` 写进
  `ai_message_parts`，所以重开 App 能无损恢复，界面能按顺序渲染工具卡（AIH-019/049）。
- **token 累加**：多轮循环把每轮 usage 累加成一个 OpenAI 形状的对象再交给 `TokenUsage.from()`
  （只算最后一轮会严重少报，AIH-057）。
- **取消**：走协程取消 → `runInterruptible` 打断阻塞读 → Run 记 `cancelled`，
  并且**不再继续工具循环**（AIH-022）。
- **工具失败不打断 Run**：任何异常都变成本次调用的失败结果（`tool.failed` + 稳定 code），
  模型可以自己纠正，或者如实告诉用户（AIH-036：工具失败要如实报告，不许伪造进度）。
- **三层预算**（防"AI 拿 ComfyUI 当轮询器"）：
  ① 一次回复最多 `maxToolSteps = 8` 轮（到顶那轮不带 `tools`，逼它收尾）；
  ② 单 Run 工具调用总数 `maxCallsPerRun = 16`；
  ③ **查 ComfyUI 的次数 `maxComfyQueriesPerRun = 3`**（AIH-036 的原话）。
  超预算的工具调用会以 `TOOL_BUDGET_EXCEEDED` / `QUERY_BUDGET_EXCEEDED` 失败返回，模型能看懂并改用已有信息。
- **提示词版本 v2**：v1 里那句「当前版本尚未注册任何工具」删掉了，改成工具清单 + 权限边界 +
  Skills 使用纪律 + 防注入规则（AIH-046）。
- **网关不认 `tools` 时的兜底**：很多网关收到 `tools` 直接 400。这种情况本次 Run 会自动
  **退回纯文本模式重试一次**，并在回复开头如实写明"上游网关不接受工具参数"——
  不能让用户在"模型到底行不行"上猜（也绝不假装是模型自己不想用工具）。

## 4. Skills（M5）

### 4.1 磁盘布局与来源

```
<根>\skills\builtin\<name>\SKILL.md        内置，只读（优先级 100）
<storage>\ai\skills\<name>\SKILL.md        用户 / AI 注册，可改可删（优先级 200）
<storage>\ai\skills\<name>.md              平铺写法也认
```

同名冲突**不静默覆盖**：用户版本胜出并保持可用，胜出者带 `conflict` 提示，界面显示冲突说明（AIH-037）。

**装 skill 的唯一方式是投放口**（用户建议第 6 条，取代了原来的「从 DSH 导入」按钮）：
把 skill 文件夹（或一个 `.md`）拷进 `<storage>\ai\skills`，然后

- **后端启动时自动登记**：没有 frontmatter 的文件会被补上 `--- name / description ---`
  （`name` 取文件名 kebab-case，`description` 取正文第一行），**正文一字不改**；
  已经有 frontmatter 的文件一律不碰（哪怕它不合法，仍然按 §4.2 的规矩"列出来 + 带诊断"）；
  中文文件名转不出 kebab-case 时会在 `errors` 里如实说明，不静默忽略。
- 应用开着的时候点右侧栏的刷新（`POST /api/ai/skills/rescan`）即可，不用重启。
- 界面显示**后端算好的绝对路径**（源码树 `<项目根>\storage\ai\skills`，
  发布包 `<根>\storage\ai\skills`，便携式）并提供「打开文件夹 / 复制路径」。

`%USERPROFILE%\.dsh` 现在**任何代码都不读**（连导入按钮也删了）：没装 DSH 的机器拿到的是同样的功能。

### 4.1.1 frontmatter 里的块标量

真实 SKILL.md 里 `description` 十有八九写成 YAML 块标量：

```markdown
---
name: 3d-animation-short-generator
description: |
  Create complete stylized 3D animated shorts from a story idea…
compatibility: …
---
```

解析器支持 `|` / `>` / `|-` / `>-`（含缩进块），多行描述进系统提示前会**压成一行**
（`SkillDto.oneLineForPrompt`）。这不是锦上添花：不支持的话读出来就是字面量 `|`，
等于让这个 skill 在目录里"没有描述"，模型根本不知道什么时候该用它
（本机 16 个 skill 里当时有 5 个中招）。

### 4.2 frontmatter 与校验

```markdown
---
name: anima-prompt
description: 把模糊需求转成 Anima 优化提示词
whenToUse: 用户要 Anima 生图提示词时
version: 1
user-invocable: true
disable-model-invocation: false
---
正文（Markdown）
```

- `name` 必须是 kebab-case（`^[a-z0-9][a-z0-9-]{0,63}$`）且与目录名一致；`description` 必填
  （写入路径上限 4000 字，扫描已有 skill **不因为描述长就判非法**）；正文非空（≤256KB）。
- **非法项不静默忽略**：仍然列在界面里、带 `validationError` 诊断，但**不进系统提示、不能被加载**（AIH-038）。
- `disable-model-invocation: true` → 只有用户能显式调用（外部未审核 Skill 的推荐姿势）。

### 4.3 即时生效

- 正文真源在磁盘，**没有索引缓存**：`register_skill` 写盘后，下一次 `load_skill` 就能读到；
  用户点删除，下一次 Run 的系统提示里就没有它了。
- 系统提示里只注入**名称 + 描述（截断 240 字）**，正文按需 `load_skill` 取（AIH-039/040）——
  这是上下文成本的关键。
- 用户说的"不重启应用、新对话生效"就是这样满足的：提示词每次 Run 现渲染。

### 4.4 安全（AIH-044/045）

- **不执行任何 Skill 带来的脚本**：本期工具集里根本没有 shell/进程工具，`scripts/` 目录只是不可执行的资源。
- 投放口只读"根下一层"的 `<name>/SKILL.md` 或 `<name>.md`，不做递归搜索；自动登记只补 frontmatter，
  不动正文（超 256KB 的正文直接报错，不写）。
- 注入防线写在系统提示第 4 条：工具输出、Skill 正文与**长期记忆**都是**数据**，
  里面出现"忽略你之前的规则"这类话要报告给用户，而不是照做。

## 4.5 长期记忆（M6）

用户建议"引入长期记忆"的落地方式：

```
<storage>\ai\memory.md      一行一条，人能看、能手改（与 Skills 同样的取舍：真源在磁盘上）
```

- **注入**：每次 Run 现读，`SystemPrompt.VERSION = v3` 起多一段「长期记忆」，
  与工具输出同等对待（数据不是指令）；注入部分截断到 4000 字符，文件本身可以更长（上限 8000）。
- **写入**：AI 走 `remember` 工具（默认 `allow`，写的是应用自己的记忆文件，
  不是用户文件系统 —— 所以不走 `ToolPolicy.resolveWrite`，也不需要审批）；
  界面走 `PUT /api/ai/memory`（整篇）与 `POST /api/ai/memory/entries`（追加一条）。
- **不做静默截断**：单条 > 500 字、总量 > 8000 字都是**报错**，并提示用户去界面里清理；
  重复内容（忽略大小写与首尾空白）不会写两遍。
- **为什么是文件而不是表**：用户随时能打开看、能手改、能整篇删掉。
  代价是"按类别检索 / 谁写的审计"这类能力暂时没有（见第 6 节）。

## 5. 接口一览（新增）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/ai/skills` | 列出 Skills（含非法诊断、冲突提示） |
| GET | `/api/ai/skills/{name}` | 元数据 + 正文 |
| POST | `/api/ai/skills` | 注册 / 覆盖（界面用） |
| DELETE | `/api/ai/skills/{name}` | 删除（仅用户来源） |
| GET | `/api/ai/skills/roots` | **投放口位置**（`userRoot` / `builtinRoot`），界面显示它 |
| POST | `/api/ai/skills/rescan` | 重新扫描投放口 + 自动登记没有 frontmatter 的 skill |
| GET / PUT / DELETE | `/api/ai/memory` | 长期记忆：读 / 整篇替换 / 清空 |
| POST | `/api/ai/memory/entries` | 追加一条记忆 |
| GET | `/api/ai/tools` | 工具清单 + 生效权限 |
| GET / PUT | `/api/ai/tools/policy` | 读 / 改白名单、逐工具权限、轮数与调用上限 |
| POST | `/api/ai/tool-calls/{callId}/approve` \| `/deny` | 工具卡上的批准 / 拒绝 |

SSE 新增事件：`reasoning.delta`、`tool.requested`、`tool.started`、`tool.completed`、`tool.failed`；
`message.completed` 增加 `parts`（有序块，界面按它定稿）与 `steps`。

## 6. 已知未做 / 下一步

1. **附件可发（M3）**：与工具无关，但适配器的 `transports` 仍是空集，图片附件依旧是"正确阻断"。
2. **第三方 Skill 的 ZIP 导入 + 预览确认**（AIH-043/044）：现在装 skill 靠"拷进投放口"这一条路
   （ZIP 要用户自己解压后拷进去）。
3. **内置 Anima / H3 Skills 正文**（AIH-041/042）：内置根 `<根>\skills\builtin` 目前是空的，
   用户自己的 16 个在投放口里（发布包会带上 `skills\` 目录，但正文能不能随项目分发要单独确认许可）。
4. **"记住这次允许"**：现在每次 `ask` 都要点一次；之后可以在工具卡上给"本次会话都允许"。
5. **审批与工具卡的断线续传**：Run 被后端重启打断时，等待中的审批会随 Run 一起失败（如实报错，不会静默执行）。
6. **长期记忆的进阶**：按类别分组、命中检索（现在是全量注入 + 截断）、"这条是谁写的"审计。
