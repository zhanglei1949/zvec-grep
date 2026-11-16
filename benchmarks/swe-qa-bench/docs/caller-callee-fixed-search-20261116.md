# 固定 search 参数的 direct 小规模 E2E v1

本轮对比刚修改前后的指导，重点检查 relationships 组能否从 Grep/read
转向 caller/callee，以及关系调用能否提供有效位置、改善评分或减少 token。
本轮只执行以下 24 个 trial，不启动 QA20 全量。

## 执行前冻结的协议

- 旧候选：`d33509a65508b66eaa55de37d8b089c1f948a109`，上一轮新版指导。
- 新候选：`85cb58345dac16ac62d85ae99365869cebcc8304`，按证据缺口选择入口与关系查询。
- 两者均显式 direct。变化包括 AGENTS.md、MCP instructions、caller/callee 描述；
  检索、图解析、关系查询实现与 search schema/原始 description 不变。
- 相同 harness、OpenCode 1.18.4、custom-openai/glm-5.2、既有 endpoint、
  temperature=0、seed=42、high reasoning、max_tokens=32000；judge 配置不变。
- 固定 QA20 原题 matplotlib:37、sympy:38、xarray:32，题面、参考答案及源码 commit 不变。
- 每候选分别运行 relationships 与 agent 两组，每题两次：2×2×3×2=24 trial。
  这是单轮诊断，不替代正式多轮评测；不与上一轮原始样本合并。
- 保留原生工具与子代理，禁用网页工具，关系工具自主选择，不在题面要求调用。
- 相同 local/potion-code-16m-v2、graph=true、suite ignore、候选支持的文件类型，
  不传文件大小覆盖参数；索引准备不计入 Agent 时间。
- 每 run 最多两个任务 job，trial 串行；基础设施异常沿用最多额外重试两次。
  保留所有低分、零调用、空返回、错误和缺失，不按效果筛选或重跑。

## 实际锁定 search

本轮所有 MCP search 的实际执行参数固定为 `preview="short"`、`limit=20`。
这两个参数是同一协议对两份候选的共同控制，适配层不修改 Rust 候选。

1. 临时本机 MCP 适配器位于 OpenCode 与原 MCP server 之间，两工具组均经过它。
2. search 的模型可见 schema 声明常量和默认值；description 同步说明固定参数，
   移除与固定策略冲突的 full 预览建议。两版的这些控制完全一致。
3. 每次 search 在转发前填入/覆盖这两个参数，其他参数不改；callers/callees 参数不改。
4. `agent/mcp-execution.jsonl` 记录每次调用的原始参数、执行参数、被规范化的字段、
   实际 MCP 返回与状态；`tool-surface.raw.json`、`tool-surface.json` 分别保存控制前后定义。
5. `model-requests.jsonl` 继续保留实际发给 LLM 的原始请求；不重写其 messages 或模型响应。
   模型提出的 full/其他 limit 尝试、客户端参数拒绝、规范化记录均单独保留。
6. 预检与 collect 校验候选专属的原始/实际 surface 指纹；执行记录必须完整闭合，
   每次转发 search 必须符合 short/20。模型前预检使用独立开发 fixture 的真实工具调用。

如果 Agent 使用 shell 绕过 MCP 调用 zg，单独核对并报告参数控制覆盖情况，不默认为受控。
relationships 组没有 search；其工具定义与参数不受该控制改变。

## 执行前定义的指标

- **采用率**：至少执行一次 callers/callees 的 trial / 每候选每组预定 6 trial。
  基础设施缺失单列，不伪装为自主零调用；分别报告两个工具及合并采用。
- **执行与非空**：分别计调用、执行成功、错误、空返回、非空返回、未知及客户端无效尝试。
  非空不直接等于准确或有效；统计所有 trial，不按是否使用工具筛选评分。
- **有效导航调用**：轨迹和冻结源码可核实，返回与题意相关的新位置或消除调用关系歧义，
  且被后续取证/回答使用。重复已知位置、空返回、错误、非空未使用和证据不足分别保留。
  以逐调用审查表说明理由；调用正确性判断不改参考答案，不作为筛除分数的条件。
  此项是轨迹/源码审查，不冒充独立盲评，也不把所有返回边都视为准确。
- **评分**：沿用现有独立 judge 与五维 rubric，逐 trial 原始分数全部保留，
  先每题求均值，再对三题等权平均。
- **Token**：完整主/子会话树，分别报告含缓存输入、未缓存输入、输出（含 reasoning）、
  总 token（含缓存输入+输出）。judge 用量不混入 Agent 用量，字符数不当作 token。
- **行为**：检查定义确认后的首次跨函数取证是否选择关系工具，以及是否减少重复 grep/read；
  核对两份指导、工具描述和 schema 在实际模型请求中可见。
- 原始全样本为主结果。任何按采用分组的结果只作描述，不能据此声称工具的因果收益。

## 复现入口与产物

在同一冻结 harness 分支分别对两候选 SHA 和两组 dispatch：

```sh
gh workflow run swe-qa-bench.yml -R zhanglei1949/zvec-grep --ref HARNESS_BRANCH \
  -f candidate_ref=CANDIDATE_SHA -f experiment_group=GROUP \
  -f experiment_phase=guidance-fixed-search -f task_set=qa20 \
  -f scope=repro-3 -f model=glm-5.2 -f embedding=local -f guidance_variant=direct
```

每个 run 保存候选包、预检、完整 evidence、task reports、summary artifacts。
模型执行后先按候选独立 aggregate，禁止混入历史候选或不同参数控制的样本。
执行后补记 harness SHA、run ID、本地归档与校验和、参数覆盖、逐调用有效性审查及结果。
