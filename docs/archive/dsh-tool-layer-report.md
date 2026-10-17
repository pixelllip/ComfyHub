# DSH 工具层复刻报告（Kotlin 后端 + Flutter 桌面端可兼容实现）

版本：`@deepseek-ai/dsh* v0.1.5-rc.2`（checkout `C:\Users\Administrator\AppData\Local\npm-cache\_npx\1e7f6d9597241db0\`）。
所有工具实现在 `node_modules\@deepseek-ai\dsh-tool-*\lib\index.js`（**未压缩 ESM**，文件顶部含 JSDoc）。

## 0. 定义载体与组合方式

- 工具通过 `defineTool({ name, description, parameters, output, timeoutMs, isConcurrencySafe, execute, presentCall, presentResult })` 注册到 `ctx.tools`。
- `parameters` **不是标准 JSON Schema**：是"每属性带 `required: true`"的自定义 DSL（`{type, required, description, enum, items, oneOf, additionalProperties}`）。转到 OpenAI 风格 `input_schema` 时必须把 `required: true` 收集成顶层 `required: []`。`output.schema` 用的是同类 DSL，另有 `render(args,value) => [{type:"text",text}]` 决定回灌模型的文本。
- 组合是三层：`dsh-base/cordis.patch.yml`（宿主）→ `dsh-web-app/cordis.patch.yml`（禁用宿主行）→ **Agent preset** `dsh-agent-presets/presets/standard/agent.cordis.yml`（`~/.dsh/profiles/web/package.json` 的 `dsh.profile.bundles` 声明）。当前会话 preset = `standard`（中文名"标准模式"）。

## 1. 当前 Windows 会话实际暴露的工具

### 1.1 文件系统（只读 / 变更）

| 工具 | 用途 | 参数（名:类型, 必填） | 触达 | Windows 特性 |
|---|---|---|---|---|
| `read` | 读 UTF-8 文本，返回带行号内容 | `file_path:string*`、`offset:number`、`limit:number` | 文件系统，只读 | 输出尾部 footer 三种：`(Output capped. Showing lines A-B. Use offset=N to continue.)` / `(Showing lines A-B of T. Use offset=N to continue.)` / `(End of file - total T lines)`；正文形如 `<path>…</path>\n<type>file</type>\n<content>\nN: text…\n</content>` |
| `write` | 创建或整体覆盖文件 | `file_path:string*`、`content:string*`、`sandbox_permissions:string`(enum)、`justification:string` | 文件系统，**变更** | 变更前必须已 `read`（`fs-observation-policy`），否则报 `cannot modify "<p>": file has not been read — read the file, then retry` |
| `edit` | 字面量替换（默认唯一匹配） | `file_path:string*`、`old_string:string*`、`new_string:string*`、`replace_all:boolean`、`sandbox_permissions`、`justification` | 文件系统，**变更** | 校验：`file_path` 非空、`old_string` 非空、`old_string !== new_string`；成功文本 `The file <p> has been updated successfully.`（replace_all 时 `… All occurrences were successfully replaced.`） |
| `read_image` | 读 PNG/JPEG/WebP/GIF 图片本体 | `file_path:string*` | 文件系统 + attachment 服务，只读 | 无扩展名也可（按内容嗅探）；需模型支持图像输入；超限报 `IMAGE_DIMENSION_TOO_LARGE` / `IMAGE_TOO_MANY_PIXELS` / `IMAGE_TOO_LARGE` |
| `glob` | 按路径模式找文件（非目录） | `pattern:string*`、`path:string` | 文件系统，只读 | 由打包的 ripgrep 二进制（`@vscode/ripgrep`）以固定 argv 直接 spawn，**不经 shell**；默认 100 条，超限会 spill 完整列表并告知路径 |
| `grep` | ripgrep 正则搜内容，按文件分组 | `pattern:string*`、`path:string`、`include:string` | 文件系统，只读 | `include` 必须是**单个**正向 glob：空串/`!` 否定/逗号列表都报错（`{a,b}` 交替合法）；默认 250 条匹配，单行预览 2000 字节 |

`read/write/edit/glob/grep` 均支持 VCS 元数据目录排除（glob），`glob/grep` 也包含隐藏/忽略文件。

### 1.2 Shell

| 工具 | 参数 | 说明 |
|---|---|---|
| `pwsh`（Windows 专用，`tool-pwsh`；Linux 上是 `bash`） | `command:string*`、`description:string*`（5–10 词，UI 显示）、`timeoutMs:number`、`workdir:string`、`run_in_background:boolean`、`sandbox_permissions:string`(enum `workspace-write`\|`danger-full-access`)、`justification:string` | 每次调用是**全新 pwsh 进程**（cwd/变量/函数不保留）；原生路径 `C:\...`；读环境变量用 `$env:NAME`；托管环境事实在 `$env:DSH_*` |
| `bash` | 同上（`description` 示例不同） | 仅 `process.platform !== 'win32'` 启用 |

关键文案（原文）：非零退出报 `[exit code: N]`；Windows 被强杀结算为 `[exit code: 1]` 且无 signal 标记，视为中断而非失败；`Long output is truncated to its tail; the full output is saved to a file whose path is reported when available.`

沙箱拒绝标记（`dsh-sandbox` 唯一词汇）：
```
[sandbox: file access denied under <mode> mode]
[sandbox: escalation available — retry this exact command once with sandbox_permissions (the narrowest wider mode that suffices) + justification; the approval prompt asks the user]
```
（文件操作把 `command` 换成 `operation`。）

**Windows 专属约束（写在 `pwsh` description 里）**：`read-only` 下 pwsh 跑 **ConstrainedLanguage**，只允许 cmdlet 与核心类型（`[string]`/`[datetime]`/`[regex]`/`[guid]`）；`[System.IO.*]::`、`[math]::`、`Add-Type`、COM、反射报 "only core types"。两种受限模式下**程序不能开命名管道**：Node.js `child_process.spawn/exec` 默认 `stdio:'pipe'` 会 EPERM，而 `stdio:'inherit'|'ignore'` 与 PowerShell 自身管道不受影响。

### 1.3 后台任务 / 子代理 / 编排

| 工具 | 参数 | 说明 |
|---|---|---|
| `job_output` | `job_id:string*`、`wait:boolean`、`timeout_ms:number` | 流式 job 只返回上次读取后的增量；每次响应尾带 `[status: ...]` |
| `job_list` | 无 | 返回 `(no background jobs)` 或每行 `<id> [<kind>] <status> — <label>` |
| `job_kill` | `job_id:string*`、`reason:string` | 立即返回，实际停止后结算为 killed |
| `subagent` | `description:string*`(3–5 词)、`prompt:string*`、`provider:string`、`model:string`、`reasoning_effort:string`、`run_in_background:boolean` | preset 配 `provider: spawn, backgroundMode: continuable` → 默认后台，返回 durable `subagentId` |
| `subagent_fork` | 同上 | `provider: fork`，继承父会话上下文，不开放模型选择（保 KV cache） |
| `send_message` | `agent_id:string*`、`message:string*` | 只投递给直接 continuable 子（resident child 可投给直接父）；**不返回答案，只回投递确认** |
| `interrupt_agent` | `agent_id:string*` | 只停当前 turn |
| `list_agents` | `scope:string`(enum `children`\|`descendants`) | 状态 running/idle/ready |
| `list_subagent_models` | `provider:string`、`model:string` | `modelSelectionSettings: true` 时注册 |
| `workflow` | `script:string*`、`meta:object*{name*,description*,whenToUse?,phases?[{title*,detail?,provider?,model?}]}` | 仅当用户明确要求 workflow/大规模编排时使用 |
| `ralph` | `objective:string*`、`maxRounds:number` | 仅当用户明确点名 Ralph；preset 配 `maxRounds: 64`，`subagentProvider: spawn` |

### 1.4 会话/交互/网络/交付

| 工具 | 参数（原文结构） |
|---|---|
| `todo_write` | `todos:array*` of `{content:string*, status:string*(enum pending\|in_progress\|completed)}`，`additionalProperties:false`；preset `allowParallelInProgress: true` |
| `ask_user_question` | `questions:array*` of `{id:string*, question:string*, header?:string, options?:array[{label:string*,description?:string}], multi_select?:boolean}`；`additionalProperties:true` |
| `get_goal` | `{}` |
| `create_goal` | `objective:string*`、`max_goal_rounds:number` |
| `update_goal` | `goal_id:string*`、`revision:number*`、`action:string*`(enum `edit`\|`pause`\|`resume`\|`complete`\|`blocked`)、`objective?:string`、`max_goal_rounds?:number`、`blocked_reason?:string` |
| `web_search` | `queries:array*` of string，1–4 条（`WEB_SEARCH_MAX_QUERIES = 4`） |
| `web_fetch` | `url:string*` |
| `present` | `files:array*` of `{path:string*, description?:string}`，1–8 个（`maxFiles` 默认 8） |
| `exit_plan_mode`（plan mode 专用） | `plan:string*`，必须 `/^#\s+\S/`（markdown 以 `#` 开头） |
| `skill` | `name:string*`（会话 skill 目录里的精确名） |

可选未启用：`str_replace_editor`（`command` enum `view|create|str_replace|insert`、`path*`、`file_text`、`insert_line`、`new_str`、`old_str`、`view_range`）、`bash`、`dsh-tool-cordis` 的 7 个 `cordis_*` 自省工具。

## 2. 工具循环协议（wire 层）

模型侧 **assistant** 消息的 tool call 块（`dsh-llm-deepseek/lib/index.js` L108–124）：

```js
const toolCalls = message.content.filter((b) => b.type === "tool-call").map((block) => ({
  id: block.id, type: "function",
  function: { name: block.name, arguments: block.arguments }
}));
return { role: "assistant", content: text,
  ...reasoning.length > 0 ? { reasoning_content: reasoning } : {},
  ...toolCalls.length > 0 ? { tool_calls: toolCalls } : {} };
```

**工具结果**在 harness 内部是 `user` 角色消息里的 `tool-result` 块（`dsh-llm` L94–107）：

```js
function createToolResultMessage(input) {
  return createUserMessage({ source: { kind: "tool", callId: input.callId },
    content: [{ type: "tool-result", toolCallId: input.callId,
                content: input.content, isError: input.isError }] });
}
```

序列化为 wire 时展开成独立 `tool` 角色消息（L149–159）：

```js
for (const result of toolResults) wire.push({
  role: "tool",
  tool_call_id: result.toolCallId,
  content: flattenText(result.content) || "(no output)"
});
```

因此：**`arguments` 是 JSON 字符串**（`function.arguments`），`tool_call_id` 对应 `tool_calls[].id`；含图片的结果后面会补一条 `user` 消息（`TOOL_RESULT_IMAGE_TEXT` + image parts）。

持久化事件：`tool/call`（`{turn, step, callId, name, arguments}`）与 `tool/result`（`{turn, step, message, error?, meta?}`，`surfaceOp:"append"`, `sourceEventSeqs:[callSeq]`）。

**错误回报**：`dsh-tools` L3490 —— 抛出的错误被统一折叠为：

```js
return { content: [{ type: "text", text: `Error: ${message}` }],
         isError: true, error: { message, ...info ? { info } : {} } };
```

超时由 `dsh-tool-call-timeout-policy` 产生 `isError`，文本 `tool call timed out after ${timeoutMs}ms`，`error.code = "TOOL_TIMEOUT"`。工具取消：`Error: tool call aborted` / `Error: tool call aborted before dispatch`。

## 3. Skills 机制

**磁盘布局**（`dsh-skill-filesystem` 的 `roots()`，按 rank 从小到大优先）：
1. `<projectRoot>/.dsh/skills`（rank 100，`projectRoot` = 向上找到含 `.git` 的目录）
2. `<projectRoot>/.agents/skills`（rank 200）
3. `customSkillDirs`（rank 300）
4. **`<dshHome>/skills`（rank 400）→ 即 `~/.dsh/skills`**，本机实测为 `C:\Users\Administrator\.dsh\skills\<name>\SKILL.md`
5. `~/.agents/skills`（rank 500，可用 `DSH_AGENTS_HOME` 覆盖）
6. bundled（rank 600，`DSH_BUNDLED_SKILL_DIR`）

单文件识别规则：`segments.length === 2 && segments[1] === "SKILL.md"`（目录型），或 `segments[0].endsWith(".md")`（平铺型）。每个 skill 的 `resourceBase = {kind:"directory", path: <SKILL.md 所在目录>}`，正文中相对路径按该目录解析。

**front-matter 格式**（`parseFrontmatter`：首行必须严格等于 `---`，行尾 `\r` 容忍；再找一行 `---` 结束；中间用 YAML 解析，body = 余下内容 `.trim()`）：

```markdown
---
name: anima-change
description: 纯指导型呈现方法论 skill——…
whenToUse: 可选
disable-model-invocation: true   # 可选，布尔
user-invocable: false            # 可选，布尔
metadata: { ... }                # 可选，任意对象
---
```

校验规则（不合法就**忽略该文件并 warn**，不抛错）：
- `name` 与 `description` 必须是非空字符串；`name` 必须匹配 `/^[a-z0-9]+(?:-[a-z0-9]+)*$/`（kebab-case）。
- `whenToUse` 可选，非空字符串。
- 布尔字段接受 `boolean`、`1`/`"1"`、`0`/`"0"`、`"true|yes|on"`、`"false|no|off"`，否则抛 `frontmatter field "<k>" must be a boolean`。
- 旧字段名被显式拒绝：`disableModelInvocation` → 用 `disable-model-invocation`；`modelInvocable` → 用 `disable-model-invocation`；`userInvocable` → 用 `user-invocable`。
- 补全后的 `invocation = {modelInvocable: disableModelInvocation !== true, userInvocable: userInvocable !== false}`。
- **未识别的额外字段不报错**（`version:`、`compatibility:` 实测存在且被忽略）。
- `dsh-skill` 侧 `validateDefinition` 复查：name 正则、description 非空、`whenToUse` 若存在必须是 string、`content` 必须 string。

**元数据如何进上下文**：`skill` 工具所在的 `tool-skill` 插件在 `agent/pre-step` 时读取 `ctx.skills.snapshot({cwd, scope})`，过滤 `isModelInvocable`，按 name 码点排序，把摘要渲染成一条 **user 消息**（不是 system prompt）：

```
<system-reminder>
A skill is a reusable set of task-specific instructions. The following skills are available in this session:

<available_skills>
- `anima-change`: <description（超长按 catalogDescriptionMaxLength 截断，默认 500）>
</available_skills>

If the user names a skill, or the task clearly matches a skill's description, call the `skill` tool with the exact skill name before taking task actions. …
</system-reminder>
```
目录变化时发出"replacement catalog"（`The available skill catalog changed. This complete catalog replaces every earlier available-skills list…`），身份由 entries 的 sha256 摘要判定。**仅当调用 agent 能解析到本插件的 `skill` 工具注册时才发布目录。**

**按需加载**：模型调用 `skill(name)` → `ctx.skills.get(name)` → 成功时结果渲染为 `renderSkillContent`：

```
<skill_content name="<escaped>">
<skill_resources>
Base directory for this skill: <dir>
Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.
</skill_resources>

<skill_instructions>
<正文>
</skill_instructions>
</skill_content>
```
错误：`invalid skill name "<n>"`、`skill "<n>" is unknown or no longer available`、`skill "<n>" is not available for model invocation`。用户显式调用 skill 时，同一 `<skill_content>` 由 `agent/pre-step` 作为 user 消息注入，模型不应再调 `skill`。

## 4. `~/.dsh/settings.yaml`

单文件，按 **顶层 namespace** 分段，YAML/JSON 均可（默认 `<harness home>/settings.yaml`，扩展名只接受 `.yaml/.yml/.json`）；热重载 `watch: true`，写入用原子 + 跨进程文件锁 + 保留注释的叶子级 diff。本机实测 namespace：

| namespace | 内容 |
|---|---|
| `permission` | `defaultPreset: <preset 名>`（来自 `dsh-permission-presets`，`PERMISSION_SETTINGS_NAMESPACE = "permission"`；schema 仅 `{defaultPreset: enum(presetChoices)}`） |
| `shell` | `SHELL_SETTINGS_NAMESPACE = "shell"`；承载 `PwshLocalExecutor.Config`：`cwd`、`timeoutMs`(默认 120000)、`maxTimeoutMs`(默认 600000)、`maxOutputBytes`(默认 64000)、`maxSpillBytes`、`graceMs`、`pwshPath` |
| `agent-default-model` | `provider`、`model`（实测 `command-code-goat` / `deepseek/deepseek-v4.1-flash`） |
| `agent-presets` | `default: standard` |
| `llm-deepseek` / `llm-pi-ai` / `web-search-deepseek` | 各自 provider 配置（`llm-pi-ai.providers.<id>.{apiKeyEnv,api,baseURL,models[]}`） |

**`permission` preset 表**（`dsh-base/cordis.patch.yml`）：`read-only → {sandbox: read-only, approval: ask}`、`workspace-write → {sandbox: workspace-write, approval: ask}`、`danger-full-access → {sandbox: danger-full-access, approval: never}`；`"custom"` 是保留名（不可作为表项）。默认走 `mode: process.env.DSH_PERMISSION_MODE ?? 'workspace-write'`，`approval.policy` 在 `danger-full-access` 时为 `never`，否则 `ask`。

**没有** `autoApprove` / `allowedDirectories` 列表：文件白名单由 `sandbox-policy` 的 `mode + workspaceRoot` 表达 —— `read-only` 零可写；`workspace-write` 允许 `policy.workspaceRoot`（= 会话 cwd）+ 宿主 `/tmp`；`danger-full-access` 不限。本会话 `permission` 段缺省，即 `sandbox = danger-full-access`（会话内 policy 行显示 "The DSH file sandbox does not restrict file modifications by available operations."）。

## 5. 限额 / 截断 / 审批总表

| 项 | 值 |
|---|---|
| `read` 默认窗口 | `readLimit = 2000` 行、`readMaxLineLength = 2000`、`readMaxBytes = 50*1024`、`readStreamMinSize = 10MB`（≥此值走流式） |
| `glob` / `grep` | `globMaxResults = 100`、`grepMaxMatches = 250`、`grepMaxLineBytes = 2000`；`sampleOverCapGlobResults: false`（超限取**修改时间序前 N**）；`timeoutMs = SEARCH_TIMEOUT_MS`；raw stdout 超 `RAW_OUTPUT_MAX_BYTES` 报 `SEARCH_RAW_OUTPUT_OVERFLOW` |
| pwsh 执行器 | `timeoutMs` 默认 120000、上限 `maxTimeoutMs` 600000（`min(args.timeoutMs ?? default, cap)`）；`maxOutputBytes` 64000 |
| **通用 spill**（`spill-policy`, `maxInlineBytes: 50000`） | 文本结果 UTF-8 字节超 50000 → 全文存 session 作用域附件，模型侧换成 head/tail 预览 + 通知：`(...omitted from ...) Full formatted result stored at: <locator> ...`（`LOCATION = " Full formatted result stored at: "`） |
| tool-result pruner | `thresholdChars: 8192`、`headChars: 4096`、`tailChars: 1024` |
| `web` | `searchMaxResults = 8`、`searchMaxQueries = 4`、`fetchMaxOutputChars = 200000`（200k 字符）、`DEFAULT_WEB_TOOL_TIMEOUT_MS = 30000`；preset 覆盖 `searchTimeoutMs: 60000` |
| `present` | `maxFiles = 8` |
| `str_replace_editor` | `maxOutputChars = 16000` |
| 并行工具调用 | `DEFAULT_MAX_PARALLEL_TOOL_CALLS = 10` |
| 子代理深度 | `maxDepth` 默认 3（`"provider-managed"` 可选） |
| 工具超时 | 工具声明 `timeoutMs`，由 `dsh-tool-call-timeout-policy` 包一层 `deadline(exec.signal, timeoutMs, "TOOL_TIMEOUT")`，命中返回 `isError` 文本 `tool call timed out after ${timeoutMs}ms` |

**审批/升级语义**（`dsh-sandbox` + `dsh-user-approval`）：`sandbox_permissions` 只在挂载了"会限制的"后端时才出现在 schema 里，枚举固定为 `ESCALATION_TARGETS = ["workspace-write","danger-full-access"]`（`read-only` 是地板，不作为升级目标）。严格更宽表：

```js
const WIDER_MODES = { "read-only": ["workspace-write","danger-full-access"],
                      "workspace-write": ["danger-full-access"] };
```

规则原文要点：只有**刚被拒绝的同一个调用**才能一次性重试升级，需带 `justification`（一句话）；请求不比当前模式更宽则**不弹审批**（甚至直接满足）；批准前不执行任何东西；升级通过 `ctx.approval` 走用户审批；`approval.policy = "never"`（即 `danger-full-access` preset）时拒绝即终态。被拒升级对该命令是终局，但不禁后续其它命令的尝试/升级。
