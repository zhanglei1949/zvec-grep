# direct 新旧指导小规模 E2E 对照 v1

本轮验证已审阅的 Rust 指导及工具说明修改是否影响关系工具采用率、回答评分和
token 消耗。只运行小样本，不启动 QA20 全量，不比较 flow。

## 固定协议

- 旧指导：`7edcbd0eec239783ce1510d1b31521e9157153ee`，显式 direct。
- 新指导：`d33509a65508b66eaa55de37d8b089c1f948a109`，显式 direct。
- 新旧差异包含 AGENTS.md、MCP instructions、工具 description/schema 的已审阅修改；
  检索引擎、图解析和关系查询实现不变。不能进一步归因到其中某一条指导。
- 两个候选使用同一 harness commit、OpenCode 1.18.4、custom-openai/glm-5.2、
  既有模型 endpoint 与采样、local/potion-code-16m-v2、启用 graph 的相同索引协议。
- 固定 QA20 原题 `matplotlib:37`、`sympy:38`、`xarray:32`，参考答案与源码版本不变。
  覆盖具名类型、概念定位及具名方法的跨函数问题；不改题面添加 symbol。
- 每个候选运行 relationships 和 agent 两组，每题 2 次，共 24 个 trial。
  这是单轮诊断，不替代正式的多轮、每题五次评测。
- Agent 保留原生工具和子代理，禁用网页工具，自主选工具，不强制调用关系工具。
- 复用历史任务的选择仅为复查已知行为；不使用历史分数代替本轮旧指导重跑。
- 每个 run 最多两个并发 task job，trial 串行。基础设施异常最多额外重试两次；
  失败、缺失、低分、零调用及空结果均保留，不按效果筛选或重跑。
- 参考答案只供独立 judge 使用，任务容器与索引不含参考答案及报告。

## 校验与指标

每个候选在模型执行前，对独立开发 fixture 运行真实 MCP 查询。
`fixtures/caller-callee-guidance-revision-v1.json` 按候选 SHA 保存两组实际
instructions、完整工具定义和安装 AGENTS.md 的联合指纹；预检与每个 trial
均校验该指纹。原有历史 diagnostic/guidance-contrast 的冻结校验不修改。

每个 trial 保存原始回答、工具调用、完整会话树用量及实际 HTTP 请求。
复核模型请求中两份指导和预期工具是否可见，区分未采用、参数错误、空结果与执行失败。

- 关系采用率：至少执行一次 callers 或 callees 的 trial 数 / 全部预定 trial 数。
  分别列出 callers、callees 的采用率、调用次数、成功、失败和空结果；无效 JSON
  尝试另外记录。基础设施失败的执行状态报告为缺失，不伪装成自主零调用。
- 评分：沿用现有逐回答 judge 与评分维度，固定 judge 配置；先求每题 trial 均值，
  再对三题等权平均。列出逐题与两组结果，不因低分排除。
- Token：完整主/子会话树，输入含未缓存输入、cache read/write，输出含 reasoning。
  同时报告总输入、未缓存输入、输出及总 token；不把字符数当作 token。
- 不应用历史离群排除规则。新旧比较要求同一组的三题均完整；若缺失则列出缺失，
  已完成数据保留，不能把部分样本标为完整对照。
- 三题两次的结果只用于确定下一步，不据此声称总体质量或成本收益显著。

## 执行入口

在同一个冻结 harness 分支上，分别对两份完整候选 SHA 与两组 dispatch：

```sh
gh workflow run swe-qa-bench.yml -R zhanglei1949/zvec-grep --ref HARNESS_BRANCH \
  -f candidate_ref=CANDIDATE_SHA -f experiment_group=GROUP \
  -f experiment_phase=guidance-revision -f task_set=qa20 \
  -f scope=repro-3 -f model=glm-5.2 -f embedding=local -f guidance_variant=direct
```

每个 run 自动保留候选包、预检、原始 evidence、task report 和 summary artifacts。
先按候选独立 aggregate，禁止将两个候选混入同一 report；跨候选对照另外生成。
执行后补记 harness SHA、run ID、失败记录、产物位置与校验和及结果。
