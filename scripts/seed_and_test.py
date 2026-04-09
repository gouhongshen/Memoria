#!/usr/bin/env python3
"""Seed 100 users × 500 memories, then compare local vs production MCP latency."""

import requests, json, time, sys, concurrent.futures, statistics

BASE = "http://localhost:8100"
MASTER_KEY = "test-master-key-for-perf-testing"
HEADERS = {"Authorization": f"Bearer {MASTER_KEY}", "Content-Type": "application/json"}

def create_api_key(user_id):
    r = requests.post(f"{BASE}/auth/keys", headers=HEADERS,
                      json={"user_id": user_id, "name": "perf-test"})
    r.raise_for_status()
    return r.json()["raw_key"]

def seed_user(user_idx):
    user_id = f"perf-user-{user_idx:04d}"
    try:
        raw_key = create_api_key(user_id)
    except Exception as e:
        print(f"  Failed key for {user_id}: {e}", file=sys.stderr)
        return None

    h = {**HEADERS, "X-User-Id": user_id}
    ok = 0
    for j in range(500):
        try:
            r = requests.post(f"{BASE}/v1/memories", headers=h, json={
                "content": f"Memory #{j} for {user_id}: discussed topic-{j%50} "
                           f"about project-{j%20}, category-{j%10}. "
                           f"Key insight: finding-{j} relates to area-{j%30}."
            }, timeout=30)
            r.raise_for_status()
            ok += 1
        except Exception as e:
            if ok == 0: print(f"  {user_id} first error: {e}", file=sys.stderr)
        if (j+1) % 100 == 0:
            print(f"  {user_id}: {j+1}/500")
    
    print(f"  {user_id}: done {ok}/500, key={raw_key[:16]}...")
    return {"user_id": user_id, "raw_key": raw_key, "count": ok}

def mcp_call(url, token, method, params=None):
    payload = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params: payload["params"] = params
    h = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}
    t0 = time.time()
    r = requests.post(url, headers=h, json=payload, timeout=60)
    lat = (time.time() - t0) * 1000
    r.raise_for_status()
    return r.json(), lat

def percentile(data, p):
    data = sorted(data)
    k = (len(data) - 1) * p / 100
    f = int(k)
    c = f + 1 if f + 1 < len(data) else f
    return data[f] + (k - f) * (data[c] - data[f])

def bench_mcp(label, url, token, runs=20):
    print(f"\n{'='*60}")
    print(f"{label}")
    print(f"  url={url}  token={token[:16]}...")
    print(f"  runs={runs}, sequential (single concurrency)")
    print(f"{'='*60}")

    # warmup: initialize
    try:
        _, lat = mcp_call(url, token, "initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "perf-test", "version": "1.0"}
        })
        print(f"  initialize: {lat:.0f}ms")
    except Exception as e:
        print(f"  initialize FAILED: {e}")
        return

    # memory_store
    store_lats = []
    for i in range(runs):
        try:
            _, lat = mcp_call(url, token, "tools/call", {
                "name": "memory_store",
                "arguments": {"content": f"Perf test memory #{i} ts={time.time()}"}
            })
            store_lats.append(lat)
            print(f"  store [{i+1:2d}]: {lat:.0f}ms")
        except Exception as e:
            print(f"  store [{i+1:2d}]: ERROR {e}")

    # memory_search
    search_lats = []
    for i in range(runs):
        try:
            _, lat = mcp_call(url, token, "tools/call", {
                "name": "memory_search",
                "arguments": {"query": f"topic-{i}"}
            })
            search_lats.append(lat)
            print(f"  search [{i+1:2d}]: {lat:.0f}ms")
        except Exception as e:
            print(f"  search [{i+1:2d}]: ERROR {e}")

    for name, lats in [("store", store_lats), ("search", search_lats)]:
        if not lats: continue
        print(f"\n  {name} ({len(lats)} samples):")
        print(f"    avg={statistics.mean(lats):.0f}ms  "
              f"p50={percentile(lats,50):.0f}ms  "
              f"p95={percentile(lats,95):.0f}ms  "
              f"p99={percentile(lats,99):.0f}ms  "
              f"min={min(lats):.0f}ms  max={max(lats):.0f}ms")

def cmd_seed():
    print(f"=== Seeding 100 users × 500 memories ===")
    t0 = time.time()
    keys = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
        futs = {pool.submit(seed_user, i): i for i in range(100)}
        for f in concurrent.futures.as_completed(futs):
            r = f.result()
            if r: keys[r["user_id"]] = r["raw_key"]
    print(f"\nDone: {len(keys)} users in {time.time()-t0:.1f}s")
    with open("/tmp/memoria-perf-keys.json", "w") as f:
        json.dump(keys, f, indent=2)
    print("Keys saved to /tmp/memoria-perf-keys.json")

def cmd_test():
    # Load local keys if available
    try:
        with open("/tmp/memoria-perf-keys.json") as f:
            keys = json.load(f)
        user_id = sorted(keys.keys())[0]
        local_token = keys[user_id]
        local_label = f"LOCAL ({user_id}, ~500 memories)"
    except FileNotFoundError:
        local_token = MASTER_KEY
        local_label = "LOCAL (master key, default user)"

    bench_mcp(local_label, f"{BASE}/mcp", local_token)
    bench_mcp("PRODUCTION (1 memory)", "https://api.thememoria.ai/mcp",
              "sk-07536ff12e1f4105aa0a68590070e00e")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python3 seed_and_test.py --seed   # Create 100 users × 500 memories")
        print("  python3 seed_and_test.py --test   # Compare local vs prod latency (20 runs each)")
        print("  python3 seed_and_test.py --seed --test")
        sys.exit(0)
    requests.get(f"{BASE}/health", timeout=5).raise_for_status()
    if "--seed" in sys.argv: cmd_seed()
    if "--test" in sys.argv: cmd_test()
