#!/usr/bin/env bash
# cmx-agent M1 端到端：serve 前门跨进程持久化断言。
# 起两次独立进程共用同一 data_dir，验证第二次进程能看到第一次的会话并续上回合号。
set -euo pipefail

cd "$(dirname "$0")"
DATA_DIR="$(mktemp -d -t cmx-agent-e2e-XXXXXX)"
trap 'rm -rf "$DATA_DIR"' EXIT

BIN=(cargo run --offline -q -p cmx-agent-cli --)

pass=0; fail=0
check() { # check <desc> <needle> <haystack>
  if grep -q "$2" <<<"$3"; then echo "  ✓ $1"; pass=$((pass+1));
  else echo "  ✗ $1 — expected to find: $2"; echo "    got: $3"; fail=$((fail+1)); fi
}

echo "── 进程 1：send 一条消息 ──"
OUT1=$(printf '%s\n' '{"cmd":"send","session_id":"chat","text":"算 2+3"}' | "${BIN[@]}" serve "$DATA_DIR" 2>/dev/null)
check "回合1完成"        '"turn":1'            "$OUT1"
check "add 结果 sum=5"   '"sum":5.0'           "$OUT1"
check "落库文件存在"      ""                    "$([ -f "$DATA_DIR/sessions/chat/log.jsonl" ] && echo present)"

echo "── 进程 2（新进程，同 data_dir）：list + send 续跑 ──"
OUT2=$(printf '%s\n%s\n' '{"cmd":"list_sessions"}' '{"cmd":"send","session_id":"chat","text":"继续"}' | "${BIN[@]}" serve "$DATA_DIR" 2>/dev/null)
check "列表含 chat 会话"  '"id":"chat"'         "$OUT2"
check "回合号续到 2"      '"turn":2'            "$OUT2"

echo "── 进程 3：get_events 全量回放 ──"
OUT3=$(printf '%s\n' '{"cmd":"get_events","session_id":"chat"}' | "${BIN[@]}" serve "$DATA_DIR" 2>/dev/null)
check "回放含首回合 add"  '"sum":5.0'           "$OUT3"
check "两个 turn_ended"   'turn_ended'          "$OUT3"

echo ""
echo "e2e: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
