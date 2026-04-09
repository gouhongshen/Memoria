# MCP Tool Metrics 设计文档

## 目标

为每个 MCP tool 接口统计分阶段耗时，满足：
1. **各阶段互斥且完整**：所有阶段耗时之和 = tool 总耗时
2. **每个 tool 有总耗时**：p50, p95, p99
3. **每个阶段有独立耗时**：p50, p95, p99
4. **并行阶段取 wall-clock 时间**（不是各分支之和）

## Metric 命名

```
memoria_tool_duration_seconds{tool="memory_search",phase="total"}     # 总耗时
memoria_tool_duration_seconds{tool="memory_search",phase="route"}     # 路由+缓存
memoria_tool_duration_seconds{tool="memory_search",phase="embed"}     # embedding HTTP
memoria_tool_duration_seconds{tool="memory_search",phase="retrieve"}  # graph∥vector 并行
memoria_tool_duration_seconds{tool="memory_search",phase="rank"}      # 排序+截断
```

## 阶段划分原则

- **串行阶段**：独立计时
- **并行阶段**：整体计时（wall-clock），不拆分内部
- **阶段之间无间隙**：t_total = t_phase1 + t_phase2 + ... + t_phaseN

---

## 各 Tool 阶段设计

### memory_search / memory_retrieve（高频，最关键）

```
total = route + embed + retrieve + rank

route:    user_sql_store + active_table（缓存命中 ~0ms）
embed:    embedding HTTP 调用
retrieve: graph ∥ vector ∥ feedback_weight 并行（取 wall-clock）
rank:     scoring + truncate + explain 构建
```

### memory_store（高频）

```
total = route + embed + dedup + insert + graph_sync

route:      user_sql_store + active_table
embed:      embedding HTTP 调用
dedup:      find_near_duplicate 向量搜索
insert:     INSERT INTO mem_memories
graph_sync: create_node + batch_upsert_entities + batch_upsert_links
```

### memory_correct

```
total = resolve + correct

resolve:  retrieve(query, 1)（仅 query 模式，id 模式跳过）
correct:  route + (get_from ∥ embed) + insert + supersede
```

注：resolve 阶段内部就是一次完整的 memory_search，会产生自己的 metrics。
correct 阶段的 `get_from ∥ embed` 是并行的，取 wall-clock。

### memory_list（高频，简单）

```
total = route + query

route: user_sql_store + active_table
query: SELECT ... LIMIT
```

### memory_profile

```
total = route + query

route: user_sql_store + active_table
query: SELECT ... WHERE memory_type = 'profile'
```

### memory_purge

```
total = route + resolve + safety_snapshot + delete + cleanup

route:           user_sql_store + active_table
resolve:         find_ids_by_topic（topic 模式）或直接用 id
safety_snapshot: create safety snapshot（best-effort）
delete:          soft_delete_batch
cleanup:         cleanup_entity_data_batch
```

### memory_feedback

```
total = route + validate + insert + update_stats

route:        user_sql_store
validate:     SELECT memory exists
insert:       INSERT INTO mem_retrieval_feedback
update_stats: UPDATE mem_memories_stats
```

### memory_snapshot (create)

```
total = quota_check + create + register + summary

quota_check: count_registrations ∥ get_reg ∥ get_reg_by_internal（并行）
create:      CREATE SNAPSHOT DDL
register:    INSERT INTO mem_snapshots
summary:     COUNT(*) time-travel query（REST API only）
```

### memory_snapshots (list)

```
total = route + list

route: user_sql_store
list:  SHOW SNAPSHOTS + list_snapshot_registrations + per-snapshot COUNT
```

### memory_rollback

```
total = resolve + restore_main + restore_aux

resolve:      resolve_snapshot_for_user
restore_main: DELETE + INSERT SELECT (mem_memories)
restore_aux:  graph_nodes ∥ graph_edges ∥ edit_log（并行）
```

### memory_governance

```
total = route + cooldown_check + quarantine + cleanup + snapshot_health

route:           user_sql_store
cooldown_check:  check_cooldown
quarantine:      quarantine_low_confidence
cleanup:         cleanup_stale
snapshot_health: list_snapshots
```

### memory_consolidate

```
total = route + cooldown_check + consolidate

route:          user_sql_store
cooldown_check: check_cooldown
consolidate:    graph consolidation（内部多步）
```

### memory_reflect

```
total = route + cooldown_check + cluster + llm + store

route:          user_sql_store
cooldown_check: check_cooldown
cluster:        build_reflect_clusters（graph 查询）
llm:            LLM chat 调用（可选）
store:          store_memory × N
```

### memory_observe

```
total = llm_extract + embed + persist + graph_sync

llm_extract: LLM 提取候选记忆（或 raw_candidates ~0ms）
embed:       embed × N（串行，无 LLM 时在 persist 内）
persist:     persist_with_dedup × N
graph_sync:  create_node + entity upsert × N
```

### memory_capabilities

```
total = 0（纯内存，无 DB/HTTP 调用）
```

不需要 metrics。

### memory_extract_entities / memory_link_entities

```
total = route + query + llm + upsert

route:  user_sql_store
query:  get_unlinked_memories / existing entities
llm:    LLM entity extraction（可选）
upsert: batch_upsert_entities + batch_upsert_links
```

---

## 实现方案

### Metric 类型

```rust
// 在 memoria-api/src/metrics/ 下新增 tool.rs
pub struct ToolMetrics {
    /// Per-tool per-phase duration histogram.
    /// Key: "tool|phase"
    pub phase_duration: HistogramVec,
}
```

Histogram buckets（覆盖 1ms 到 30s）：
```rust
const TOOL_DURATION_BOUNDS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];
```

### 记录方式

在每个 tool 的代码路径中插入计时：

```rust
let t0 = Instant::now();
// ... route ...
let t_route = t0.elapsed();
metrics.phase_duration.observe("memory_search|route", t_route.as_secs_f64());

let t1 = Instant::now();
// ... embed ...
let t_embed = t1.elapsed();
metrics.phase_duration.observe("memory_search|embed", t_embed.as_secs_f64());

// ... etc ...

metrics.phase_duration.observe("memory_search|total", t0.elapsed().as_secs_f64());
```

### Prometheus 输出

```
# HELP memoria_tool_duration_seconds MCP tool execution duration by phase.
# TYPE memoria_tool_duration_seconds histogram
memoria_tool_duration_seconds_bucket{tool="memory_search",phase="total",le="0.1"} 5
memoria_tool_duration_seconds_bucket{tool="memory_search",phase="total",le="0.5"} 18
...
memoria_tool_duration_seconds_bucket{tool="memory_search",phase="embed",le="0.1"} 12
...
```

### Grafana 面板

每个 tool 一个 row，包含：
- **总耗时** p50/p95/p99 时间序列
- **阶段分布** stacked bar（各阶段占比）
- **阶段明细** 每个阶段的 p50/p95/p99

查询示例：
```promql
histogram_quantile(0.95, rate(memoria_tool_duration_seconds_bucket{tool="memory_search",phase="total"}[5m]))
```

---

## 优先级

1. **P0**: memory_search, memory_store, memory_list（高频热路径）
2. **P1**: memory_correct, memory_snapshot, memory_rollback
3. **P2**: memory_feedback, memory_governance, memory_purge
4. **P3**: memory_reflect, memory_observe, memory_extract_entities
