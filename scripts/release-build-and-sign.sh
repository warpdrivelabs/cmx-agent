#!/usr/bin/env bash
#
# release-build-and-sign.sh —— cmx-agent Tauri 桌面壳的「编译 → 签名 → 公证 → 装订 → 打 dmg」一条龙。
#
# 产物：
#   src-tauri/target/release/bundle/macos/TrueMate.app        （签名+公证+stapled）
#   src-tauri/target/release/bundle/macos/TrueMate.app.tar.gz （updater 产物：stapled .app 重打包 + minisign .sig）
#   src-tauri/target/release/bundle/TrueMate.dmg              （含 app + Applications 软链接，首装分发）
#
# 前置（一次性，本机已就绪；换机器需重做）：
#   1) Developer ID Application 证书 + 私钥已导入登录钥匙串
#      （`security find-identity -v -p codesigning` 能看到 1 valid identity）
#   2) 公证凭据已存进 keychain profile：
#      xcrun notarytool store-credentials "cmx-agent-notary" \
#        --apple-id <AppleID> --team-id <TEAMID> --password <专用密码>
#   3) tauri CLI 可用：`npm install @tauri-apps/cli`（本脚本默认用 /tmp/node_modules/.bin/tauri，
#      可用 TAURI_CLI 环境变量覆盖）。
#
# 用法：
#   ./release-build-and-sign.sh             # 全流程
#   ./release-build-and-sign.sh --no-build  # 跳过编译，只对已有 .app 做签名+公证+dmg
#   ./release-build-and-sign.sh --no-notarize  # 只签名+打 dmg，不公证（仅自用调试）
#
# 退出码非 0 即失败；任一步失败即中止。
set -euo pipefail

# ── 配置（按需改） ────────────────────────────────────────────────
IDENTITY="Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)"
TEAM_ID="W8H2ZU6LLY"
NOTARY_PROFILE="cmx-agent-notary"
APP_NAME="TrueMate"
BUNDLE_ID="com.pansoft.cmx-agent"
TAURI_CLI="${TAURI_CLI:-/tmp/node_modules/.bin/tauri}"
# 自动更新（方案 §6.1/§8）：minisign 私钥（仓库外！）+ 密码所在的钥匙串服务名。
MINISIGN_KEY="${MINISIGN_KEY:-$HOME/.tauri/cmx-agent.key}"
KEYCHAIN_SERVICE="cmx-agent-updater-key"

# ── 路径 ─────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC_TAURI="$SCRIPT_DIR/../crates/cmx-agent-shell/src-tauri"
BUNDLE_DIR="$SRC_TAURI/target/release/bundle/macos"
APP="$BUNDLE_DIR/$APP_NAME.app"
DMG="$SRC_TAURI/target/release/bundle/TrueMate.dmg"
TAR_GZ="$BUNDLE_DIR/$APP_NAME.app.tar.gz"   # updater 产物（重打包后的，见第 6 步）

# ── 参数 ─────────────────────────────────────────────────────────
DO_BUILD=1
DO_NOTARIZE=1
for arg in "$@"; do
  case "$arg" in
    --no-build)    DO_BUILD=0 ;;
    --no-notarize) DO_NOTARIZE=0 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
  esac
done

log() { printf '\n\033[1;36m▶ %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

# ── 1. 前置检查 ──────────────────────────────────────────────────
log "前置检查"
[ -x "$TAURI_CLI" ] || TAURI_CLI="$(command -v tauri || true)"
[ -n "$TAURI_CLI" ] || die "找不到 tauri CLI（设 TAURI_CLI 或 npm install -g @tauri-apps/cli）"
echo "tauri CLI: $TAURI_CLI"
security find-identity -v -p codesigning | grep -q "$IDENTITY" \
  || die "钥匙串中找不到签名身份『$IDENTITY』（检查证书+私钥是否导入）"
echo "签名身份: OK"
[ "$DO_NOTARIZE" -eq 1 ] && {
  # notarytool profile 存在 iCloud 钥匙串里，`security find-generic-password` 查不到；
  # 用 notarytool history 实际验证 profile 可用。
  xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
    || die "notarytool profile『$NOTARY_PROFILE』不可用（xcrun notarytool store-credentials）"
  echo "公证 profile: OK"
}

# ── 2. 编译 ──────────────────────────────────────────────────────
# updater 签名 env 先行：tauri.conf.json 开了 createUpdaterArtifacts，
# 缺 TAURI_SIGNING_PRIVATE_KEY(_PATH) 时 `tauri build` 直接失败。
# 密码从登录钥匙串取（生成时已存，见 scripts/README.md「自动更新密钥」节）。
log "导出 updater 签名密钥（minisign）"
[ -f "$MINISIGN_KEY" ] || die "minisign 私钥不存在：${MINISIGN_KEY}（用 tauri signer generate 生成，放仓库外）"
# 注意：`tauri build` 打 updater 产物只认 TAURI_SIGNING_PRIVATE_KEY（密钥**内容**），
# 不认 _PATH 变体（signer sign 子命令才认 -f/env _PATH），两个都导出兼容。
export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH TAURI_SIGNING_PRIVATE_KEY_PASSWORD
TAURI_SIGNING_PRIVATE_KEY="$(cat "$MINISIGN_KEY")"
TAURI_SIGNING_PRIVATE_KEY_PATH="$MINISIGN_KEY"
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(security find-generic-password -s "$KEYCHAIN_SERVICE" -w)" \
  || die "钥匙串取不到 ${KEYCHAIN_SERVICE}（security add-generic-password 补录，见 README）"
echo "minisign 私钥: $MINISIGN_KEY"
if [ "$DO_BUILD" -eq 1 ]; then
  log "tauri build（release，约几分钟）"
  # cargo clean -p：UI 资产内嵌进壳二进制，增量编译不重嵌（既有坑）——每次发布强制重编壳 crate，
  # 确保 src-tauri/ui/（sync-ui.sh 生成物）真的是打进 .app 的那份。
  ( cd "$SRC_TAURI" && cargo clean -p cmx-agent-shell && "$TAURI_CLI" build )
fi
[ -d "$APP" ] || die "未找到产物: ${APP}（先去掉 --no-build 跑一次编译）"

# ── 3. 签名 ──────────────────────────────────────────────────────
log "codesign（deep + hardened runtime + timestamp）"
codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
# 注意：codesign -dv 输出很大，用 grep -q 配 pipefail 会触发 SIGPIPE（rc=141）。
# 故先落临时文件再 grep，避免管道破裂被 set -e 误判。
codesign -dv --verbose=4 "$APP" > /tmp/cmx-codesign-dv.out 2>&1 \
  || die "codesign 验证失败"
grep -q "Authority=Apple Root CA" /tmp/cmx-codesign-dv.out \
  || die "签名后证书链未到 Apple Root CA"
echo "签名: OK"

# ── 4. 公证 + 装订 ───────────────────────────────────────────────
if [ "$DO_NOTARIZE" -eq 1 ]; then
  log "打包 zip 并提交 Apple 公证（--wait，约几分钟）"
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  ditto -c -k --keepParent "$APP" "$WORK/cmx-agent.zip"
  xcrun notarytool submit "$WORK/cmx-agent.zip" \
    --keychain-profile "$NOTARY_PROFILE" --wait

  log "stapler staple（装订公证票据）"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
fi

# ── 5. Gatekeeper 终检 ───────────────────────────────────────────
log "spctl --assess（Gatekeeper 终检）"
spctl --assess --verbose=4 "$APP" > /tmp/cmx-spctl.out 2>&1 \
  || die "Gatekeeper 未 accepted（见 /tmp/cmx-spctl.out）"
grep -q "accepted" /tmp/cmx-spctl.out \
  || die "Gatekeeper 未 accepted"
echo "Gatekeeper: accepted"

# ── 6. updater 产物（.app.tar.gz 重打包 + minisign） ─────────────
# 为什么重打包：tauri build 自产的 .app.tar.gz 打包自**未经本脚本重签/公证的 .app**，
# 直接分发会让更新后的应用被 Gatekeeper 拦（方案 §6.1 / R9）。正确产物 = 用走完
# 签名→公证→staple 的最终 .app 重新打 tar.gz（staple 票据随 .app 走，离线可验），
# 再用 minisign 签名（客户端 updater 验的就是这个 .sig，公钥在 tauri.conf.json）。
log "重打包 updater 产物 TrueMate.app.tar.gz + minisign 签名"
# COPYFILE_DISABLE=1：macOS bsdtar 默认会把扩展属性存成 ./_ AppleDouble 条目，
# updater 解包时对「跳过首段后路径为空」的 ._ 条目直接 unpack 失败（实测踩坑）。
# tauri bundler 原生（Rust tar crate）不产 ._ 条目，重打包必须对齐。
( cd "$BUNDLE_DIR" && rm -f "$APP_NAME.app.tar.gz" "$APP_NAME.app.tar.gz.sig" \
    && COPYFILE_DISABLE=1 tar -czf "$APP_NAME.app.tar.gz" "$APP_NAME.app" )
# env -u：上面为 tauri build 导出了内容型 TAURI_SIGNING_PRIVATE_KEY（映射 -k），
# 与这里的 -f 路径型互斥（clap 冲突报错），sign 前去掉，用 -f + 密码 env。
env -u TAURI_SIGNING_PRIVATE_KEY "$TAURI_CLI" signer sign -f "$MINISIGN_KEY" "$TAR_GZ"
[ -f "$TAR_GZ.sig" ] || die "minisign 签名未产出: $TAR_GZ.sig"
echo "updater 产物: OK（stapled .app 重打包 + minisign）"

# ── 7. 打 dmg ────────────────────────────────────────────────────
log "hdiutil 打 dmg（含 Applications 软链接）"
STAGING="$(mktemp -d)"
# WORK 仅在公证分支赋值；此处统一清理，避免 set -u 报 unbound。
trap 'rm -rf "${WORK:-}" "$STAGING"' EXIT
cp -Rp "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" \
  -fs HFS+ -format UDBZ "$DMG"

# ── 8. 完成 ──────────────────────────────────────────────────────
log "完成 ✅"
echo "  app: $APP"
echo "  dmg: ${DMG}（首装分发，门户下载页用）"
echo "  updater: ${TAR_GZ}（+ .sig，进更新源；latest.json 的 signature 填 .sig 文件内容）"
echo "  app 大小: $(du -sh "$APP" | awk '{print $1}')"
echo "  dmg 大小: $(du -sh "$DMG" | awk '{print $1}')"
