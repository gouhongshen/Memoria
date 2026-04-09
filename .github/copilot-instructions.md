# Copilot Instructions — Memoria Memory Integration

You have persistent memory via MCP tools (`memoria` server). Memory survives across conversations.

## 🔴 Every conversation start

Call `memory_retrieve` with a **semantic query** derived from the user's message BEFORE responding.

- ✅ Extract key concepts → `"benchmark optimization"`, `"graph retrieval bug"`
- ❌ Don't use meta-queries → `"all memories"`, `"everything"`

After retrieval: use as reference, verify against current context. No results is normal for new topics.

## 🔴 Every question answered — MUST store conclusion

**This is the most important rule.** After you explore, analyze, or answer any user question, you MUST store the conclusion as a memory. No exceptions.

The workflow is: **user asks → you explore → you answer → you store the conclusion**.

```
# User: "这个项目的数据库连接池是怎么配的？"
# You: (explore code, find answer, reply to user)
# Then IMMEDIATELY:
memory_store(content="DB connection pool config: main pool from DB_MAX_CONNECTIONS env, auth pool default 16, background rebuild pool 2, entity pool 4. Configured in memoria-storage/src/store.rs:230-287", memory_type="semantic")
```

### What to store

Every answer you give contains a **distilled conclusion**. Extract and store it:

| User asks | You store |
|-----------|----------|
| "X 模块怎么工作的？" | X 模块的工作原理（一句话总结 + 关键细节） |
| "这个 bug 的原因是什么？" | Bug root cause + 修复方案 |
| "A 和 B 哪个方案好？" | 选了哪个、为什么 |
| "怎么部署这个服务？" | 部署步骤摘要 |
| "这段代码有什么问题？" | 发现的问题 + 建议 |

### How to write good memory content

- **一条记忆 = 一个结论**，不要把多个不相关的发现塞进一条
- **包含关键细节**："连接池默认 16" 比 "连接池有配置" 有用得多
- **包含位置信息**（如果有）："在 store.rs:230" 帮助下次快速定位
- **用陈述句**："X uses Y for Z"，不要用疑问句

### What NOT to store

Only skip storing when the answer is **trivially obvious** or **pure chitchat**:
- "好的" / "明白了" — 纯确认
- "这个文件在 src/main.rs" — 一眼能看到的路径
- Greetings, thanks, small talk

**When in doubt, store it.** Deduplication is automatic — storing a slightly redundant memory costs nothing, but forgetting a useful conclusion means next session has to re-explore from scratch.

## 🔴 During exploration — store intermediate findings

For multi-step exploration, store findings as you go, don't wait until the end:

1. **假设阶段** → 存为 `working`：
   ```
   memory_store(content="假设：性能瓶颈在 vector index rebuild", memory_type="working")
   ```

2. **验证后** → 结论提升为 `semantic`，清理 working：
   ```
   memory_store(content="确认：rebuild 耗时因全表扫描，加 WHERE 条件后从 12s→0.3s", memory_type="semantic")
   memory_purge(topic="假设：性能瓶颈", reason="已验证并提升为 semantic")
   ```

3. **假设被否定** → 直接清理：
   ```
   memory_purge(memory_id="...", reason="假设不成立，实际瓶颈在网络")
   ```

**原则：存结论和决策，不存过程动作**（"发现了 X" ✅ vs "看了文件" ❌）

## 🔴 User decisions and corrections

- User stated a preference or decision → `memory_store` immediately
- User corrected a previously stored fact → `memory_correct`（不要 store + purge）

## Memory types

| Type | Use for | Examples |
|------|---------|---------|
| `semantic` | 确认的事实、技术决策（默认，最常用） | "Uses MatrixOne as primary DB" |
| `profile` | 用户偏好和身份 | "Prefers concise answers with code first" |
| `procedural` | 流程和方法 | "Deploy with: make dev-start" |
| `working` | 未验证的假设、当前任务临时上下文 | "假设：embedding 精度问题在 tokenizer" |
| `episodic` | 会话总结 | "Session: 优化了 retrieval pipeline, latency 降 40%" |

## Tool quick reference

| Tool | When | Key params |
|------|------|-----------|
| `memory_store` | **每次回答问题后** / 新发现 / 用户决策 | `content`, `memory_type`, `session_id` |
| `memory_retrieve` | 会话开头 / 需要上下文 | `query`, `top_k`(default 5) |
| `memory_search` | 浏览记忆 | `query`, `top_k`(default 10) |
| `memory_correct` | 更正已存的记忆 | `query` or `memory_id`, `new_content`, `reason` |
| `memory_purge` | 删除/清理 | `memory_id`(逗号分隔批量) or `topic`, `reason` |
| `memory_profile` | 了解用户画像 | — |
| `memory_feedback` | 标记检索结果质量 | `memory_id`, `signal`(useful/irrelevant/outdated/wrong) |

## Session wrap-up

When conversation ends:
1. Clean up completed task's working memories: `memory_purge(topic="...", reason="task complete")`
2. Promote any working memory that became a lasting fact to `semantic`
3. Store an episodic summary:
   ```
   memory_store(
     content="Session Summary: [topic]\n\nActions: [what]\n\nOutcome: [result]",
     memory_type="episodic"
   )
   ```

## Anti-patterns

- ❌ **回答了问题但不存记忆**（最严重的违规 — 下次得重新探索）
- ❌ 存过程动作而非结论（"看了文件" vs "发现了 X"）
- ❌ 会话结束一次性存 10 条（应该边回答边存）
- ❌ 留下已完成任务的 working 记忆（污染未来检索）
- ❌ 同一事实既存 working 又存 semantic（选一个）
- ❌ 不确定的观察直接存为 semantic（先存 working，确认后再提升）
