# cmx-agent/scripts

cmx-agent 工程的发布/运维脚本。

## release-build-and-sign.sh

把 cmx-agent Tauri 桌面壳打包成可分发的 macOS `.app` / `.dmg`：Developer ID 签名 + Apple 公证 + 装订 + Gatekeeper 终检 + 打 dmg。一条龙，任一步失败即中止。

### 用法

```bash
cd cmx-agent/scripts
./release-build-and-sign.sh               # 全流程：编译→签名→公证→装订→Gatekeeper 终检→打 dmg
./release-build-and-sign.sh --no-build    # 复用已有 release 产物，只重签+公证+dmg
./release-build-and-sign.sh --no-notarize # 只签名+dmg，不公证（自用调试，过不了别人机器 Gatekeeper）
```

环境变量可覆盖：

- `TAURI_CLI`：tauri CLI 路径，默认 `/tmp/node_modules/.bin/tauri`。系统级装了就 `TAURI_CLI=$(which tauri) ./release-build-and-sign.sh`。

### 产物

- `.app`：`crates/cmx-agent-shell/src-tauri/target/release/bundle/macos/cmx 企业桌面智能体.app`
- `.dmg`：`crates/cmx-agent-shell/src-tauri/target/release/bundle/cmx-企业桌面智能体.dmg`

### 前置条件（一次性，本机已就绪；换机器需重做）

1. **Developer ID Application 证书 + 私钥**在登录钥匙串。验证：
   ```bash
   security find-identity -v -p codesigning
   # 能看到：Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)，1 valid identities found
   ```
2. **公证凭据**存进 keychain profile `cmx-agent-notary`。验证：
   ```bash
   xcrun notarytool history --keychain-profile cmx-agent-notary   # 不报错即可
   ```
   首次设置（用你自己的 Apple ID + 专用密码，专用密码在 appleid.apple.com 生成）：
   ```bash
   xcrun notarytool store-credentials "cmx-agent-notary" \
     --apple-id <AppleID邮箱> --team-id W8H2ZU6LLY --password <专用密码>
   ```
3. **tauri CLI** 可用：`npm install -g @tauri-apps/cli`（或脚本默认的 `/tmp` 本地装法）。

### 脚本内置默认值（顶部可改）

```bash
IDENTITY="Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)"
TEAM_ID="W8H2ZU6LLY"
NOTARY_PROFILE="cmx-agent-notary"
APP_NAME="TrueMate"
```

### 流程拆解（脚本内部步骤）

1. 前置检查：tauri CLI / 签名身份 / 公证 profile 是否就绪
2. `tauri build`（release，约几分钟）
3. `codesign` deep + hardened runtime + timestamp，校验链到 `Authority=Apple Root CA`
4. `notarytool submit --wait` 公证（等 `status: Accepted`）
5. `stapler staple` + `validate` 装订
6. `spctl --assess` Gatekeeper 终检（应 `accepted` / `source=Notarized Developer ID`）
7. `hdiutil` 打 dmg（含 `/Applications` 软链接，拖拽安装）

### 常见坑（已修进脚本，记录防回退）

- **`codesign -dv | grep -q` 在 `set -euo pipefail` 下触发 SIGPIPE（rc=141）被误判失败** → 改成先写临时文件再 grep。
- **`notarytool store-credentials` 的 profile 存在 iCloud 钥匙串，`security find-generic-password` 查不到** → 用 `xcrun notarytool history --keychain-profile` 实际验证可用性。
- **签名报 `unable to build chain to self-signed root` / `errSecInternalComponent`** → 多半是证书被人为加了自定义 trust settings。修法：`security delete-trusted-cert` 清掉 Developer ID 相关证书的自定义信任设置，回归系统默认（`security dump-trust-settings` 与 `dump-trust-settings -d` 里不应再出现 Developer ID）。证书链系统自带，不用手动装。
- **`security find-identity` 显示多条重复同一证书** → p12 多次导入导致。`while security delete-identity -c "<证书CN>" ~/Library/Keychains/login.keychain-db; do :; done` 删干净后重导一次 p12。

### 运行时依赖（非打包问题，分发时注意）

- **登录门依赖门户后端**：app 启动后登录对接 `127.0.0.1:8080`（cmx-portal-server）。给别人用需改 `AuthConfig` 指向远程门户，或目标机器也跑门户。
- **model.json 不随 app 走**：模型配置在用户各自的 `~/Library/Application Support/com.pansoft.cmx-agent/`，首次跑无配置 → 回退 demo。要让 app 自带默认模型得改代码。

## SKILL.md

本目录的 `SKILL.md` 是同名 Claude Code skill 的工程内副本（主副本在 `.claude/skills/release-cmx-agent/SKILL.md`）。改流程时改其中一份，同步另一份，避免漂移。
