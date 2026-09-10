#!/usr/bin/env bash
# 发布桌面前端：把唯一真源 crates/cmx-agent-web/ui/ 整目录同步到 Tauri 壳 crates/cmx-agent-shell/src-tauri/ui/。
# 双壳单份：SPA 拆分后的 index.html + css/ + js/ 两壳共用（call() 双桥 + Windows/Linux 自绘窗口形态，
# 后者默认 display:none、仅 Tauri 壳内 initPlatformChrome 启用，浏览器中无害）；cmx.png 真源在仓根
# images/（Web 壳 include_bytes! 内嵌进二进制，Tauri 壳 frontendDist 需要实体文件）。
# 改 UI 只改 cmx-agent-web/ui/，改完跑本脚本发布；src-tauri/ui/ 是生成物，禁止手改。
# 用法: ./sync-ui.sh
set -euo pipefail

cd "$(dirname "$0")"

SRC=crates/cmx-agent-web/ui
DST=crates/cmx-agent-shell/src-tauri/ui

[ -f "$SRC/index.html" ] || { echo "✗ 真源缺失: $SRC/index.html"; exit 1; }
# 整目录拷贝（css/ js/ 子目录 + index.html），先清目标侧旧文件再复制。
rm -rf "$DST"
mkdir -p "$DST"
cp -r "$SRC/." "$DST/"
echo "✓ $SRC/ → $DST/（整目录含 css/ js/）"

[ -f images/cmx.png ] || { echo "✗ 真源缺失: images/cmx.png"; exit 1; }
cp images/cmx.png "$DST/cmx.png"
echo "✓ images/cmx.png → $DST/cmx.png"
