fn main() {
    // 打包注入默认门户地址（编译期烧进 exe，用户装完零配置）：
    //   来源优先级 = 环境变量 CMX_AGENT_PORTAL_DEFAULT > 仓库根 backend/cmx-agent/.env 的 CMX_AGENT_PORTAL_BASE。
    // main.rs 经 option_env! 读取（cargo:rustc-env 传导）。改 .env 会自动触发重编译。
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    // src-tauri → cmx-agent-shell → crates → cmx-agent（仓库根）。
    let root_env = std::path::Path::new(&manifest).join("../../../.env");
    let portal = std::env::var("CMX_AGENT_PORTAL_DEFAULT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| read_env_file(&root_env, "CMX_AGENT_PORTAL_BASE"));
    if let Some(v) = portal {
        println!("cargo:rustc-env=CMX_AGENT_PORTAL_DEFAULT={v}");
    }
    println!("cargo:rerun-if-changed={}", root_env.display());
    println!("cargo:rerun-if-env-changed=CMX_AGENT_PORTAL_DEFAULT");
    // 登录页注册入口显隐（缺省 true）：.env / env 的 CMX_AGENT_REGISTER_ENABLED 设 false|0|off|no 可关。
    let reg_enabled = std::env::var("CMX_AGENT_REGISTER_ENABLED")
        .ok()
        .or_else(|| read_env_file(&root_env, "CMX_AGENT_REGISTER_ENABLED"))
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "off" | "no"
            )
        })
        .unwrap_or(true);
    println!("cargo:rustc-env=CMX_AGENT_REGISTER_ENABLED={reg_enabled}");
    println!("cargo:rerun-if-env-changed=CMX_AGENT_REGISTER_ENABLED");
    tauri_build::build()
}

/// 极简读 env 文件里某个 key（KEY=value，# 注释）。
fn read_env_file(path: &std::path::Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        (line.starts_with(key) && line[key.len()..].starts_with('='))
            .then(|| line[key.len() + 1..].trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}
