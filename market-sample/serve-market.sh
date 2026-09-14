#!/usr/bin/env bash
# serve-market.sh —— 本地插件市场样例服务器：把 market-sample/ 起在 http://127.0.0.1:8601。
# 用法：
#   ./market-sample/serve-market.sh          # 前台跑（Ctrl-C 停）
#   CMX_AGENT_PLUGIN_MARKET=http://127.0.0.1:8601/marketplace.json <启动应用>
# 之后应用「插件」面板的市场区即显示 marketplace.json 里的条目，可一键安装（热注册）。
set -euo pipefail
cd "$(dirname "$0")"
echo "插件市场: http://127.0.0.1:8601/marketplace.json"
echo "（Ctrl-C 停止）"
exec python3 -m http.server 8601 --bind 127.0.0.1
