---
name: release-cmx-agent
description: Build, codesign, notarize, staple, DMG-package, and produce the updater artifact (.app.tar.gz + minisign .sig) of the cmx-agent Tauri desktop app for macOS distribution. Use when the user asks to 打包/发布/release/签名/公证/出dmg/发更新 the cmx-agent Tauri shell (crates/cmx-agent-shell), or wants a distributable .app/.dmg or auto-update artifact of the desktop agent. Runs the one-shot script scripts/release-build-and-sign.sh; only fall back to manual codesign/notarytool steps if the script itself is broken.
---

# release-cmx-agent

打包 cmx-agent Tauri 桌面壳成可分发的 macOS `.app` / `.dmg` + 自动更新产物（Developer ID 签名 + Apple 公证 + 装订 + updater `.app.tar.gz`）。有一条已调通的脚本，优先用它。

## 产物路径

- `.app`：`cmx-agent/crates/cmx-agent-shell/src-tauri/target/release/bundle/macos/TrueMate.app`
- `.dmg`：`cmx-agent/crates/cmx-agent-shell/src-tauri/target/release/bundle/TrueMate.dmg`（首装分发，门户下载页用）
- updater 产物：同目录 `TrueMate.app.tar.gz` + `.sig`（进更新源静态目录；`latest.json` 的 `signature` 填 `.sig` 文件**完整文本内容**）

## 一键脚本（首选）

```bash
cd /Users/javier/Documents/workspace/rust/cmx-agent/scripts
./release-build-and-sign.sh               # 全流程：编译→签名→公证→装订→Gatekeeper 终检→updater 产物→打 dmg
./release-build-and-sign.sh --no-build    # 复用已有 release 产物，只重签+公证+updater+dmg
./release-build-and-sign.sh --no-notarize # 只签名+updater+dmg，不公证（自用调试，过不了别人机器 Gatekeeper）
```

脚本自带前置检查（tauri CLI / 签名身份 / 公证 profile / minisign 密钥），缺啥报啥。任一步失败即中止，不产半成品。默认 `TAURI_CLI=/tmp/node_modules/.bin/tauri`、`MINISIGN_KEY=~/.tauri/cmx-agent.key`，可用环境变量覆盖（如 `TAURI_CLI=$(which tauri) ./...`）。

**改完代码后的标准流程**：直接 `./release-build-and-sign.sh`（不带 `--no-build`），十几分钟出签名公证过的 dmg + updater 产物。

## 前置条件（本机已就绪；换机器需重做）

1. **Developer ID Application 证书 + 私钥**在登录钥匙串：
   `security find-identity -v -p codesigning` 能看到
   `Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)` 且 `1 valid identities found`。
2. **公证凭据**存进 keychain profile `cmx-agent-notary`：
   `xcrun notarytool history --keychain-profile cmx-agent-notary` 不报错。
   （首次设置：`xcrun notarytool store-credentials "cmx-agent-notary" --apple-id <AppleID> --team-id W8H2ZU6LLY --password <专用密码>`，专用密码在 appleid.apple.com 生成。）
3. **tauri CLI** 可用（脚本默认 `/tmp/node_modules/.bin/tauri`，或 `npm install -g @tauri-apps/cli`）。
4. **自动更新 minisign 密钥**：私钥 `~/.tauri/cmx-agent.key`（仓库外，双备份，丢了 = 永远无法再发更新），密码存登录钥匙串 `security find-generic-password -s cmx-agent-updater-key -w` 能取到。公钥在 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`。

## 流程拆解（脚本内部步骤，仅在脚本坏了时手动照做）

1. **导出 updater 签名 env**（`tauri.conf.json` 开了 `createUpdaterArtifacts`，缺 `TAURI_SIGNING_PRIVATE_KEY_PATH/_PASSWORD` 构建直接失败；密码从钥匙串取）
2. **编译**：`cd src-tauri && tauri build`（release，约几分钟）
3. **签名**：`codesign --force --deep --options runtime --timestamp --sign "Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)" "<app>"`，校验链到 `Authority=Apple Root CA`
4. **公证**：`ditto -c -k --keepParent "<app>" /tmp/cmx-agent.zip` → `xcrun notarytool submit /tmp/cmx-agent.zip --keychain-profile cmx-agent-notary --wait`（等 `status: Accepted`）
5. **装订**：`xcrun stapler staple "<app>"` + `xcrun stapler validate "<app>"`
6. **Gatekeeper 终检**：`spctl --assess --verbose=4 "<app>"` 应 `accepted` / `source=Notarized Developer ID`
7. **updater 产物**：删 tauri 自产 `.app.tar.gz`（打包自未签名 .app，废品）→ `cd bundle/macos && tar -czf TrueMate.app.tar.gz TrueMate.app` 重打包（staple 票据随 .app 走）→ `tauri signer sign -f ~/.tauri/cmx-agent.key TrueMate.app.tar.gz` 产 `.sig`
8. **打 dmg**：staging 目录放 `.app` + `/Applications` 软链接 → `hdiutil create -volname "TrueMate" -srcfolder <staging> -fs HFS+ -format UDBZ <out.dmg>`

## 常见坑（已踩并修进脚本，记录在此以防回退）

- **`codesign -dv | grep -q` 在 `set -euo pipefail` 下触发 SIGPIPE（rc=141）被误判失败** → 脚本里改成先写临时文件再 grep。手动验证时用 `codesign -dv --verbose=4 "<app>" 2>&1 | grep Authority`（不带 `set -e` 的交互式 shell 没问题）。
- **`notarytool store-credentials` 的 profile 存在 iCloud 钥匙串，`security find-generic-password` 查不到** → 用 `xcrun notarytool history --keychain-profile <name>` 实际验证可用性，别用 find-generic-password。
- **签名报 `unable to build chain to self-signed root` / `errSecInternalComponent`** → 多半是证书被人为加了自定义 trust settings，破坏了 codesign 的信任链评估。修法：`security delete-trusted-cert` 清掉 Developer ID 相关证书的自定义信任设置，回归系统默认（`security dump-trust-settings` 与 `dump-trust-settings -d` 里不应再出现 Developer ID）。证书链本身（Apple Root CA → Developer ID Certification Authority → Developer ID Application）系统自带，不用手动装。
- **`security find-identity` 显示多条重复同一证书** → p12 多次导入导致，私钥 ACL 错乱。`while security delete-identity -c "<证书CN>" ~/Library/Keychains/login.keychain-db; do :; done` 删干净后重导一次 p12。
- **tauri 自产 `.app.tar.gz` 不能直接进更新源** → 它打包自未经重签/公证的 .app，更新装上会被 Gatekeeper 拦。必须用最终 stapled .app 重打包 + minisign（脚本第 7 步已做）。
- **重打包必须 `COPYFILE_DISABLE=1`** → bsdtar 的 `._*` AppleDouble 条目会让 updater 解包报 `failed to unpack '._TrueMate.app'`（实测踩坑，脚本已带）。
- **updater endpoint 必须 https** → release 构建启动即校验，http 直接 panic。P0 联调在 `tauri.conf.json` 开 `dangerousInsecureTransportProtocol: true`；生产切 https 时必须删。
- **minisign env 两副面孔**：`tauri build` 只认内容型 `TAURI_SIGNING_PRIVATE_KEY`；`signer sign -f` 与内容型 env 互斥（脚本用 `env -u` 处理）。
- **bash 3.2 变量后接全角字符**：`$VAR（` 会把中文吞进变量名报 unbound，写 `${VAR}（`。

## 运行时依赖（分发时心里有数，非打包问题）

- **登录门依赖门户后端**：app 启动后登录对接 `127.0.0.1:8080`（cmx-portal-server）。给别人用需改 `AuthConfig` 指向远程门户，或目标机器也跑门户。
- **model.json 不随 app 走**：模型配置在用户各自的 `~/Library/Application Support/com.pansoft.truemate/`，首次跑无配置 → 回退 demo。要让 app 自带默认模型得改代码。
