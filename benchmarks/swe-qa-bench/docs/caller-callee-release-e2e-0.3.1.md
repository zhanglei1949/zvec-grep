# 官方 0.3.1 Caller/Callee E2E 协议

目标：比较组合组和 Search 组的回答评分、token 与耗时，观察关系组如何使用
caller/callee。三组保留 OpenCode 原生工具和子代理，禁用 webfetch/websearch。

| 组别 | zg MCP 工具 |
|---|---|
| search | search |
| agent（组合） | search、callers、callees |
| relationships | callers、callees |

固定 QA20 原题、参考答案、仓库 commit、index.ignore；OpenCode 1.18.4、GLM-5.2、
temperature=0、seed=42、thinking=true、reasoning_effort=high、max_tokens=32000。
使用 HTTP `/mcp`、CPU Potion Code 16M v2，模型 revision 和三个权重/分词文件的
SHA-256 写入验收。正式答题自主选择工具，每题每组每轮5次，共3轮、900次。
每组最多两个任务作业，单作业串行；每轮答题、评审结束后进入下一轮。

## 发布包和指导

从官方 npm registry 获取 `@zvec/zvec-grep@0.3.1` 与
`@zvec/zvec-grep-linux-x64-gnu@0.3.1`。保存 registry 元数据和原始 tgz，检查
SHA-512 SRI、SHA-1、包名和版本，逐项核对平台包 checksums.json；容器安装后
再次核对原生文件和 `zg --version`。使用平台包内原始二进制及配套库，不安装旧版
`@zvec/bindings-linux-x64`。源码来源预期为发布记录中的 `40b6005b`；包完整性校验
与源码来源预期分别记录。

发布版安装统一使用 `--mcp-toolset agent --mcp-transport http`。评测代理过滤
tools/list 并拒绝隐藏工具调用；从 AGENTS.md 的 zg 管理块及 MCP instructions
删除引用不可用工具的整条指导，其他内容逐字保留。组合组保留完整指导。
保留工具描述/schema 和调用参数；不展开 `$ref`、转换 globs、补引号或固定 search 参数。

每次运行使用独立容器，配置、数据库和会话不共享。仅共享模型文件及每个串行 worker
的 Node 安装缓存。图索引按仓库 commit、两个发布包摘要、ignore、CPU 配置绑定，
同一输入只构建一个快照；其余运行恢复独立副本，恢复前检查文件摘要与就绪状态。

## 技术冒烟与停止规则

三组各一个独立开发 fixture 会话，明确要求主 Agent 与 explore 子代理实际调用本组
工具；关系查询带 `globs="lib.rs"`。安装后还进行直接 MCP 查询，核对已知函数链的
返回位置。直接探针调用与 Agent 调用分开统计。

在模型 HTTP 边界检查：首个答题请求只有 system/user，实际包含安装指导和 MCP
instructions，工具集合与 schema 符合分组，生成参数符合协议。完整保留请求、SSE
响应、MCP 请求/响应和递归会话。三组均完成且实际调用通过，才允许进入 QA20。
指导、schema 或参数解析失败时，技术冒烟标为失败，正式评测保持未启动。

准备阶段最多重试两次，保留每次记录。Harbor 的 Agent 重试关闭；模型开始答题后
不因超时、低分、未调用或空结果重跑。同一工具、参数和错误连续出现3次且没有成功
工具执行时终止该次运行；1800秒总时限保留。失败、缺失及用量不完整单列。

## 统计和证据

五维自评沿用 correctness、completeness、relevance、clarity、coherence，每维1–20。
Judge 只接收题目、参考答案和最终回答，保存原始请求与响应，最多尝试3次；重新评分
不重新答题。参考答案始终位于宿主评审环境，不能进入容器或索引。

逐题报告15次得分均值和标准差，20题等权汇总，同时给出完成率、分轮结果和失败清单。
完整配对题用于组间比较；不完整题和已知用量继续保留，不能当作零分或零消耗。
输入分为总输入、未缓存输入、缓存读取/写入；输出包含 reasoning。统计主会话及所有
递归子会话，安装/索引和评审开销另列，未知费用保持未知。

主要比较 agent−search，补充 relationships−search、agent−relationships。
按题配对重采样10,000次、seed=42，报告差值和变化率的95%百分位区间。
Requests:16、Astropy:38、Conan:19、Django:21、SymPy:38、Xarray:32、Matplotlib:37
作为预先指定的7题补充分析，不另行重跑。

调用轨迹分别记录模型选择、JSON 合法性、到达 MCP、执行成功、返回位置等状态。
位置正确性、问题相关性及是否被后续阅读/回答使用，须结合冻结源码逐条复核；自动
位置清单的待核查项保持 pending，不能当作“有效调用”。调用分层用于解释轨迹，
不把自然选择工具的样本视为随机处理组。

## 执行方式

先运行本地离线测试（仅回环假服务，无模型请求）：

```sh
uv run --project benchmarks/swe-qa-bench python -m unittest discover \
  -s benchmarks/swe-qa-bench/tests -p 'test_release*.py' -v
```

Actions 入口（会先技术冒烟，通过后运行正式评测）：

```sh
gh workflow run swe-qa-bench.yml --ref <冻结的harness分支> -f release_e2e=true
```

也可在具有 Docker、锁定依赖和 GLM_API_KEY 的 Linux 环境中执行：

```sh
python -m zg_bench.release_e2e fetch-release --output /tmp/qa20-release-package
python -m zg_bench.release_e2e run \
  --release /tmp/qa20-release-package/release.json \
  --output /tmp/qa20-release-results --execute-full
```

不传 `--execute-full` 时只做技术冒烟。输出目录必须不存在，避免混合或覆盖旧轮。
Actions artifact `qa20-release-0.3.1-<run>-<attempt>` 保留协议、包元数据、tgz、
smoke-gate.json、逐次原始证据、分轮结果和 SHA-256 清单。历史结果保持原样。
