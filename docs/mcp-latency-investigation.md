# Memoria MCP 延迟排查报告

日期: 2026-04-09

## 现象

生产环境 `api.thememoria.ai` 的 MCP 接口（memory_store / memory_search）延迟极高，
单条记忆的用户 store 平均 5s，search 平均 5.3s，p95 达 13s。

## 测试环境

| 环境 | MO 实例 | MO account | memoria 模式 |
|------|---------|-----------|-------------|
| 本地 | freetier-01.cn-hangzhou (cn-qa) | 01998024... | **single-db** |
| 生产 | freetier-01.cn-hangzhou (cn-qa) | 019d076b... | **multi-db** |

两个 account 在同一个 MO 实例上，资源配置一样。

## explain=analyze 分段延迟对比

| 阶段 | 本地 | 生产 | 倍数 |
|------|------|------|------|
| embedding_ms | 86ms | 191ms | 2.2x |
| vector_ms | 341ms | **3566ms** | **10.5x** |
| graph_ms | 600ms | **2263ms** | **3.8x** |
| total_ms | 1058ms | **6150ms** | **5.8x** |

## 裸 SQL 对比

直接在两个 account 上执行相同的向量搜索 SQL：

| 查询类型 | 生产 account | 本地 account |
|---------|-------------|-------------|
| mem_memories 向量搜索 | 130-190ms | 146-186ms |
| graph_nodes 向量搜索 | 179-274ms | - |
| graph_edges 查询 | 190-253ms | - |
| user_registry 路由查询 | 178-237ms | - |

**结论：单条 SQL 延迟两个 account 基本一致，都在 130-270ms 范围。**

## 根因

**不是 MO 单条查询慢，而是一次 retrieve 串行执行了大量 SQL round-trip，每条 ~200ms，累积到 3-5 秒。**

一次 `memory_search` (retrieve_inner) 的 SQL 调用链：

### graph 阶段 (graph_ms=2263ms, 约 10+ 条串行 SQL)
1. `count_user_nodes` — 1 SQL
2. `search_nodes_vector` — 1 SQL (graph 向量搜索)
3. `search_nodes_fulltext` — 1 SQL (graph BM25)
4. `find_entities_by_names` — 1 SQL (NER entity recall)
5. `get_memories_by_entities` — 1 SQL
6. `SpreadingActivation.propagate` × 3 iterations:
   - `get_edges_bidirectional` — 每次 1 SQL
   - `get_edges_for_nodes` — 每次 1 SQL (out-degree)
7. `get_node_by_memory_id` — N 条 (per entity memory)
8. `get_nodes_by_ids` — 1 SQL (batch fetch candidates)

### vector 阶段 (vector_ms=3566ms, 约 5+ 条串行 SQL)
9. `get_user_retrieval_params` — 1 SQL (feedback weight)
10. `search_hybrid_from_scored` — 内部包含向量搜索 + fulltext + feedback batch 等多条 SQL

**总计约 15-20 条串行 SQL × ~200ms/条 ≈ 3-6 秒**

## 关键差异：本地 vs 生产

本地 memoria 使用 **single-db** 模式，生产使用 **multi-db** 模式。
本地 graph 阶段只有 600ms 而生产 2263ms，原因待进一步确认（见下方待排查项）。

## 优化建议

1. **并行化 SQL**：graph vector search / fulltext search / entity recall 互不依赖，可并行
2. **减少 spreading activation round-trip**：3 次迭代 × 2 SQL/次 = 6 条串行 SQL，可合并
3. **减少 graph 阶段 SQL 数量**：batch 化 get_node_by_memory_id
4. **评估 multi-db 的 USE db 切换开销**
5. **考虑 graph 阶段提前退出**：数据量小时跳过 spreading activation

## 为什么本地 memoria 没有这个问题？

验证结果：本地切换到 multi-db 模式后，50 条记忆的 search 延迟仍然只有 185-591ms，
和 single-db 模式差不多。**multi-db 模式本身不是问题。**

真正的差异是 **网络 RTT**：

| 链路 | 单次 TCP RTT | 单条 SQL 端到端 |
|------|-------------|---------------|
| 本地机器 → MO | **10ms** | **50-80ms** |
| 生产服务器 → MO | **估算 ~100ms** | **200-280ms** |

一次 retrieve 串行执行 15-20 条 SQL：
- 本地：15 × 70ms ≈ **1 秒**
- 生产：15 × 250ms ≈ **3.5-5 秒**

**RTT 放大效应**：每多 1ms 的网络延迟，在 15-20 次串行 SQL 下被放大 15-20 倍。

补充验证：
- 生产 `initialize`（无 DB）：120ms → 纯网络延迟（你到生产 API）
- 生产 `memory_list`（1 条简单 SQL）：400ms → 120ms 网络 + 280ms SQL
- 生产 `memory_search`（15-20 条串行 SQL）：2600-9000ms

## 结论

根因是 **高 RTT × 大量串行 SQL round-trip 的乘数效应**。
本地没问题是因为到 MO 只有 10ms RTT，生产服务器到 MO 的 RTT 高得多。

## 验证：持久连接内 SQL 执行时间

在同一个 mysql 持久连接内（模拟连接池），两个 account 的 SQL 执行时间几乎一样：

| 查询 | 生产 account | 本地 account |
|------|-------------|-------------|
| 简单 SELECT LIMIT 1 | 25-31ms | 25-26ms |
| 向量搜索 (l2_distance) | 32-39ms | - |

**MO 服务端执行时间只有 25-40ms**，但通过 mysql CLI 新建连接测出 120-270ms，
说明 TCP 建连 + 认证握手开销约 100-230ms。

## 验证：本地 multi-db vs 生产 explain 对比

两边都是 graph_hit=False（graph 未命中，约 4-5 条串行 SQL）：

| 阶段 | 本地 multi-db | 生产 | 每条 SQL 估算 |
|------|-------------|------|-------------|
| graph_ms | 50-198ms | 603-1014ms | 本地 10-40ms, 生产 120-200ms |
| vector_ms | 69-185ms | 342-873ms | 本地 15-37ms, 生产 70-175ms |

## 生产服务器位置

- `api.thememoria.ai` 前面有 **Cloudflare CDN**（边缘节点 NRT/东京）
- 源站部署在一台服务器上（docker-compose），通过 nginx 反代
- MO 在 **阿里云杭州**（118.31.75.184, Hangzhou Alibaba/Aliyun）
- 镜像仓库用的 `registry.cn-hangzhou.aliyuncs.com`

**生产服务器的具体位置未知**，但从延迟推算，服务器到 MO 的 RTT 远高于同区域部署。

## 最终结论

**根因：生产 memoria 服务器到 MO 的每条 SQL round-trip 延迟约 120-200ms，
而本地到同一 MO 实例只要 10-40ms。**

MO 服务端执行时间只有 25-40ms，额外的 80-160ms 来自网络 RTT。
一次 memory_search 串行执行 15-20 条 SQL，RTT 被放大 15-20 倍：
- 本地：15 × 30ms ≈ 450ms ✅
- 生产：15 × 160ms ≈ 2400ms，加上波动到 3-6 秒 ❌

## 优化建议（按优先级）

1. **降低生产服务器到 MO 的网络延迟** — 确认两者在同一可用区/VPC，这是最直接的解法
2. **并行化 SQL** — graph 阶段的 vector/fulltext/entity 三路查询可并行（预计 -40% 延迟）
3. **减少 spreading activation round-trip** — 3 次迭代 × 2 SQL = 6 条串行，可合并或减少迭代
4. **batch 化小查询** — get_node_by_memory_id 等逐条查询改为批量
5. **考虑 graph 提前退出** — 0 edges 时跳过 spreading activation

## 补充验证：MO 响应时间不稳定

通过交替调用 memory_capabilities (0 SQL) 和 memory_list (3 SQL) 精确测量：

| 指标 | memory_capabilities (0 SQL) | memory_list (3 SQL) |
|------|---------------------------|-------------------|
| 延迟范围 | 115-121ms | 221-1795ms |
| 波动 | <7ms | **1574ms** |
| 推算 per-SQL | - | 34ms ~ 559ms |

memory_capabilities 极其稳定，说明 CDN→源站链路没问题。
memory_list 波动巨大，说明 **MO freetier 实例的响应时间极不稳定**。

34ms 是连接池热连接的正常值（和持久连接内测出的 25-40ms 一致），
559ms 说明有时连接失效需要重建，或 MO 侧查询执行波动。

## 根因总结（修订）

两个因素叠加：

1. **MO freetier 实例响应不稳定** — 单条 SQL 延迟从 34ms 波动到 559ms（16 倍），
   这是共享资源争抢导致的，不是 memoria 代码问题
2. **串行 SQL 放大效应** — 一次 retrieve 15-20 条串行 SQL，每条的波动被累加，
   最坏情况 20 × 559ms = 11 秒

## 最终优化建议（按优先级）

1. **MO 实例升级** — freetier 共享实例是根因，升级为独占实例可消除波动
2. **部署同区** — 源站到 MO 单程 ~35ms，部署到同一可用区可降到 <1ms
3. **并行化 SQL（已实现）** — graph ∥ vector、graph 内部三路并行、batch entity upsert、correct 内 get_from ∥ embed
4. **减少 spreading activation round-trip** — 0 edges 时跳过 3 轮迭代
5. **连接池调优** — 增加 min_connections 保持热连接，减少重建开销
