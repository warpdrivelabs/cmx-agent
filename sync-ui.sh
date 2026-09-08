#!/usr/bin/env bash
# 发布桌面前端：把唯一真源 crates/cmx-agent-web/ui/ 同步到 Tauri 壳 crates/cmx-agent-shell/src-tauri/ui/。
# 双壳单份：index.html / login.html 两壳共用（call() 双桥 + Windows/Linux 自绘窗口形态，后者默认
# display:none、仅 Tauri 壳内 initPlatformChrome 启用，浏览器中无害）；cmx.png 真源在仓根 images/
# （Web 壳 include_bytes! 内嵌进二进制，Tauri 壳 frontendDist 需要实体文件）。
# 改 UI 只改 cmx-agent-web/ui/，改完跑本脚本发布；src-tauri/ui/ 是生成物，禁止手改。
# 用法: ./sync-ui.sh
set -euo pipefail

cd "$(dirname "$0")"

SRC=crates/cmx-agent-web/ui
DST=crates/cmx-agent-shell/src-tauri/ui

for f in index.html login.html; do
  [ -f "$SRC/$f" ] || { echo "✗ 真源缺失: $SRC/$f"; exit 1; }
  cp "$SRC/$f" "$DST/$f"
  echo "✓ $SRC/$f → $DST/$f"
done

[ -f images/cmx.png ] || { echo "✗ 真源缺失: images/cmx.png"; exit 1; }
cp images/cmx.png "$DST/cmx.png"
echo "✓ images/cmx.png → $DST/cmx.png"
