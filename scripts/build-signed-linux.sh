#!/usr/bin/env bash
# TrueMate Linux 签名构建（deb 更新产物 + .sig）。
#
# 用法（仓库根 backend/cmx-agent/ 下）：
#   ./scripts/build-signed-linux.sh                 # 交互输入密码
#   ./scripts/build-signed-linux.sh <密码>          # 脚本传入
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD=... ./scripts/build-signed-linux.sh
#   TAURI_SKIP_CLEAN=1 ./scripts/build-signed-linux.sh   # 跳过 clean（更快，仅限未改 UI 真源时）
#
# 私钥位置（gitignore 已挡）：crates/cmx-agent-shell/src-tauri/.tauri/cmx-agent.key
# 产物：crates/cmx-agent-shell/src-tauri/target/release/bundle/deb/*.deb + 同名 .sig
#       （deb + .sig 一对，一起上传门户维护页面登记发布）

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
src_tauri="$repo_root/crates/cmx-agent-shell/src-tauri"
key_path="$src_tauri/.tauri/cmx-agent.key"

if [[ ! -f "$key_path" ]]; then
    echo "找不到私钥：$key_path —— 把 cmx-agent.key 放到该位置后重试（勿入 git，.gitignore 已挡）" >&2
    exit 1
fi

key_password="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
if [[ -z "$key_password" ]]; then
    if [[ $# -ge 1 ]]; then
        key_password="$1"
    else
        read -rsp "输入私钥密码: " key_password
        echo
    fi
fi

cd "$src_tauri"

# release 构建默认 clean：防增量编译不重嵌 UI 真源（sync-ui.sh 改过 UI 后必须 clean）。
if [[ "${TAURI_SKIP_CLEAN:-0}" != "1" ]]; then
    cargo clean -p cmx-agent-shell
fi

# 内容型 env 全版本通吃（npx 拉的 CLI 可能不认 PATH 型）；PATH 型一并设置兼容新版。
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$key_path")"
export TAURI_SIGNING_PRIVATE_KEY_PATH="$key_path"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$key_password"

npx -y @tauri-apps/cli build

# 防漏签自检：deb 产物旁必须产出同名 .sig（缺了说明签名步没跑，勿分发）。
sigs=$(find target/release/bundle/deb -maxdepth 1 -name "*.sig" 2>/dev/null || true)
if [[ -z "$sigs" ]]; then
    echo "未找到 .sig —— 更新产物没有签名，禁止分发；检查私钥/密码后重跑" >&2
    exit 1
fi

echo
echo "构建完成（deb + .sig 成对，一起上传门户维护页面登记发布）："
echo "$sigs" | sed 's/^/  /'
find target/release/bundle/deb -maxdepth 1 -name "*.deb" | sed 's/^/  /'
