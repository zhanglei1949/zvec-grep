# G3.0 类型、校验与执行契约

历史运行记录已迁入[归档索引](archive/pre-g4-validation/README.md)；文中的旧 reports 路径指归档内位置，机器记录链接的标题保留原始路径。

日期：2026-10-28。已完成查询基础契约；尚未接入真实图查询、CLI／MCP 图工具或图增强检索。G3.1 接入前仍须确认 G2 的业务模型、TextMap 与兼容发布视图。

## 改动

| 位置 | 内容 |
| --- | --- |
| `zg-knowledge/src/query` | 中立查询参数、节点／视图版本、命中与来源证据、TextMap、Provider 接口、预算／取消／期限；不依赖 engine 或 graphdb |
| `zg-engine/src/api/context/graph` | 默认构建可用的传输类型；启用 knowledge-graph 后显式转换为 KG 类型，并用对照测试检查语义一致 |
| `ContextOptions` | 增加默认 Off 的 graph 参数；Off 序列化时省略，保持旧请求形状 |
| 执行与授权入口 | 参数先校验；rg＋图模式拒绝；G3.0 中有效的非 Off 请求返回 unsupported，不读取工作区、刷新、加载模型或提示远程授权 |

关键约束：

- 节点、引用和命中消费时核对版本；ViewStamp 只是可序列化标识，KgReadView 保留宿主租约，不能由客户端反序列化。
- ResolvedScope 没有反序列化或公开构造入口。后续真实 resolver 完成完整性与映射检查后才可产生范围；本轮不以空白名单代替未实现的范围计算。
- RequestContext 的克隆及子阶段共享一份预算和取消状态；计数溢出／超限时整次扣款拒绝，不留下部分扣款。子阶段只能收紧截止时间。
- 图阶段超时与请求总超时分开；只有 Augment 且文本安全、请求未停止时可降级。Constrain、请求取消、总超时不降级。
- 参数默认扩展两跳；Augment 最多五种子、两跳；其他探索显式跳数上限为八。路径准入与结果筛选分开，空边界和无限制分别表达。
- UTF-8 位置校验针对同一份解码文本；TextMap 检查逐段字节映射，不增加 G2 持久化格式。

## 验证

| 检查 | 结果 |
| --- | --- |
| 新增 KG 契约测试 | 12 通过：参数、跨版输入、证据、映射、租约、共享预算／并发／溢出、取消与期限 |
| 新增 engine 契约测试 | 默认构建 3 通过；开启 knowledge-graph 后 4 通过，增加传输／KG 转换与校验对照；两轮重叠不相加 |
| 严格范围不可反序列化 | 1 项 compile-fail 文档测试通过 |
| 既有查询单测／daemon 协议 | 分别 10／6 通过 |
| 既有 KG 事实契约／关系测试 | 分别 1／5 通过 |
| 普通检索真实存储回归 | 1 通过：构建、检索、更新和删除；沙箱内 PermissionDenied，沙箱外复跑通过 |
| 默认 workspace all-targets check | 通过 |
| KG 纯库编译、生产依赖树 | 通过；engine 默认不依赖 KG，KG 正常／构建依赖不含 graphdb 或 zvec；测试仍沿用既有原生开发依赖 |
| KG all-targets Clippy，-D warnings | 通过 |
| engine 严格 Clippy | 被既有 `workspace/mod.rs:12` 的 CommonMark 注释告警阻断；该文件未改。将 doc_markdown 保留为 warning 后，其余检查通过，不算作严格检查通过 |
| 改动文件 rustfmt／空白检查 | 通过 |

测试使用独立 target 与 ccache，未争用 G2 构建目录。原生依赖首次编译因沙箱不允许写全局 ccache 失败，改用临时缓存后完成。未运行 graphdb feature 的 G2 故障矩阵，不以本轮结果宣称 G2 完成或图查询已可用。

实际命令、日志及源码／保护文件摘要见 [验证记录](archive/pre-g4-validation/README.md#g3-0-contracts "包内：knowledge/reports/g3.0-contracts/verification.json")。开始时记录的 35 个保护文件（含 G2 图存储、构建／发布／恢复、读取会话及 Cargo 配置）在本轮结束时保持不变。未提交或推送。
