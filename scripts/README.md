# cmx-agent/scripts

cmx-agent 工程的发布/运维脚本。

## release-build-and-sign.sh

把 cmx-agent Tauri 桌面壳打包成可分发的 macOS `.app` / `.dmg` / updater 产物：Developer ID 签名 + Apple 公证 + 装订 + Gatekeeper 终检 + `.app.tar.gz` 重打包（自动更新用）+ minisign 签名 + 打 dmg。一条龙，任一步失败即中止。

### 用法

```bash
cd cmx-agent/scripts
./release-build-and-sign.sh               # 全流程：编译→签名→公证→装订→Gatekeeper 终检→updater 产物→打 dmg
./release-build-and-sign.sh --no-build    # 复用已有 release 产物，只重签+公证+updater+dmg
./release-build-and-sign.sh --no-notarize # 只签名+updater+dmg，不公证（自用调试，过不了别人机器 Gatekeeper）
```

环境变量可覆盖：

- `TAURI_CLI`：tauri CLI 路径，默认 `/tmp/node_modules/.bin/tauri`。系统级装了就 `TAURI_CLI=$(which tauri) ./release-build-and-sign.sh`。
- `MINISIGN_KEY`：updater minisign 私钥路径，默认 `~/.tauri/cmx-agent.key`。

### 产物

- `.app`：`crates/cmx-agent-shell/src-tauri/target/release/bundle/macos/TrueMate.app`（签名+公证+stapled）
- `.app.tar.gz`：同目录 `TrueMate.app.tar.gz` + `.sig`——**自动更新产物**。用最终 stapled .app 重打包（tauri build 自产的那份来自未签名 .app，已在本脚本第 6 步覆盖），minisign 签名供客户端 updater 验签。进更新源静态目录，`latest.json` 的 `signature` 字段填 `.sig` 文件的**完整文本内容**
- `.dmg`：`crates/cmx-agent-shell/src-tauri/target/release/bundle/TrueMate.dmg`（首装分发，门户下载页用）

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
4. **自动更新 minisign 密钥**（方案 §8）：私钥 `~/.tauri/cmx-agent.key`（仓库外，双备份！私钥丢 = 永远无法再发更新），密码存登录钥匙串：
   ```bash
   # 一次性生成（私钥 + 公钥；公钥贴进 src-tauri/tauri.conf.json 的 plugins.updater.pubkey）
   tauri signer generate -w ~/.tauri/cmx-agent.key
   # 密码存钥匙串（脚本运行时从这里取）
   security add-generic-password -a "$USER" -s cmx-agent-updater-key -w '<私钥密码>'
   # 验证可取
   security find-generic-password -s cmx-agent-updater-key -w
   ```

### 脚本内置默认值（顶部可改）

```bash
IDENTITY="Developer ID Application: Pansoft Company Limited (W8H2ZU6LLY)"
TEAM_ID="W8H2ZU6LLY"
NOTARY_PROFILE="cmx-agent-notary"
APP_NAME="TrueMate"
MINISIGN_KEY="$HOME/.tauri/cmx-agent.key"
KEYCHAIN_SERVICE="cmx-agent-updater-key"
```

### 流程拆解（脚本内部步骤）

1. 前置检查：tauri CLI / 签名身份 / 公证 profile 是否就绪
2. 导出 minisign 签名 env（`tauri.conf.json` 开了 `createUpdaterArtifacts`，缺 env 构建会失败）
3. `tauri build`（release，约几分钟）
4. `codesign` deep + hardened runtime + timestamp，校验链到 `Authority=Apple Root CA`
5. `notarytool submit --wait` 公证（等 `status: Accepted`）
6. `stapler staple` + `validate` 装订
7. `spctl --assess` Gatekeeper 终检（应 `accepted` / `source=Notarized Developer ID`）
8. **updater 产物**：删 tauri 自产 `.app.tar.gz`（打包自未签名 .app，废品）→ 用 stapled .app `tar -czf` 重打包 → `tauri signer sign` 生成 `.sig`
9. `hdiutil` 打 dmg（含 `/Applications` 软链接，拖拽安装）

### 常见坑（已修进脚本，记录防回退）

- **`codesign -dv | grep -q` 在 `set -euo pipefail` 下触发 SIGPIPE（rc=141）被误判失败** → 改成先写临时文件再 grep。
- **`notarytool store-credentials` 的 profile 存在 iCloud 钥匙串，`security find-generic-password` 查不到** → 用 `xcrun notarytool history --keychain-profile` 实际验证可用性。
- **签名报 `unable to build chain to self-signed root` / `errSecInternalComponent`** → 多半是证书被人为加了自定义 trust settings。修法：`security delete-trusted-cert` 清掉 Developer ID 相关证书的自定义信任设置，回归系统默认（`security dump-trust-settings` 与 `dump-trust-settings -d` 里不应再出现 Developer ID）。证书链系统自带，不用手动装。
- **`security find-identity` 显示多条重复同一证书** → p12 多次导入导致。`while security delete-identity -c "<证书CN>" ~/Library/Keychains/login.keychain-db; do :; done` 删干净后重导一次 p12。
- **tauri 自产 `.app.tar.gz` 不能直接进更新源** → 它打包自未经脚本重签/公证的 .app，更新装上会被 Gatekeeper 拦（R9）。脚本第 8 步已用 stapled .app 重打包覆盖；别图省事直接拿 build 产物。
- **重打包必须 `COPYFILE_DISABLE=1`** → macOS 自带 bsdtar 会把扩展属性存成 `._*` AppleDouble 条目，updater 解包时对「跳过首段后路径为空」的 `._TrueMate.app` 条目直接报错 `failed to unpack '._TrueMate.app'`（实测 0.2.0 升级装失败就是这个）。tauri bundler 原生（Rust tar crate）不产 `._` 条目，重打包必须对齐，脚本第 8 步已带。
- **updater endpoint 必须 https** → release 构建下插件启动即校验，http endpoint 直接 panic（`The configured updater endpoint must use a secure protocol`）。P0 本机联调（`http://127.0.0.1:8901`）在 `tauri.conf.json` 开 `dangerousInsecureTransportProtocol: true` 放行；**生产切 https 时必须删掉这个开关**。
- **minisign 签名 env 两副面孔** → `tauri build` 打 updater 产物只认 `TAURI_SIGNING_PRIVATE_KEY`（密钥**内容**），不认 `_PATH` 变体；而 `signer sign` 的 `-f`/`_PATH` 与内容型 env 互斥（clap 冲突报错）。脚本处理：build 前导出内容型，sign 前 `env -u TAURI_SIGNING_PRIVATE_KEY` 用 `-f`。
- **bash 3.2 下 `$VAR` 后紧跟全角字符（中文括号等）会把后续字节吞进变量名** → 报 `unbound variable`（如 `$DMG（首装...）`）。macOS 自带 bash 3.2，字符串里变量后接中文一律写 `${VAR}（`。

### 运行时依赖（非打包问题，分发时注意）

- **登录门依赖门户后端**：app 启动后登录对接 `127.0.0.1:8080`（cmx-portal-server）。给别人用需改 `AuthConfig` 指向远程门户，或目标机器也跑门户。
- **model.json 不随 app 走**：模型配置在用户各自的 `~/Library/Application Support/com.pansoft.truemate/`，首次跑无配置 → 回退 demo。要让 app 自带默认模型得改代码。

## SKILL.md

本目录的 `SKILL.md` 是 release-cmx-agent skill 的唯一副本（原「主副本」`.claude/skills/release-cmx-agent/` 已不存在）。改流程时直接改 `SKILL.md`，保持与本 README 一致，避免漂移。
