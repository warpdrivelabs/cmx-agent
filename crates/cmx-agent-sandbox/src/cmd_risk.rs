//! S3 命令级风险屏（方案 §3.5）：把高危闸从「工具级」下探到「命令级」。
//!
//! 机制：实现 core [`Guard`]（pre 相），对 `shell` 工具的 `cmd` 入参做**最小破坏模式集**
//! 静态屏，命中且非 DangerFullAccess 时升 `NeedApproval`（不硬拒——误报只多一次审批）。
//! 先例：Claude Code 的 Bash 权限规则前缀匹配。
//!
//! **双面定位写死（红队 B-14）**：这是审批策略层，不冒充 OS 隔离；漏报清单——base64 管道
//! 解码、`$( )`/反引号命令替换、别名/函数、工作区内脚本内嵌命令，静态正则天然全瞎。
//! UI 文案不得暗示该屏是防线（本守卫只产出审批卡，无独立 UI 文案）。

use cmx_agent_core::guard::{Guard, GuardCtx, GuardDecision, GuardPhase};

/// 最小破坏模式集（十余条；正则刻意保守——宁可漏报不可误伤日常工作流）。
/// 单元级用纯函数 [`scan`]；这里注册守卫形态。
pub struct CmdRiskGuard;

impl Guard for CmdRiskGuard {
    fn name(&self) -> &str {
        "cmd_risk"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        let s = crate::settings::get();
        if !s.cmd_risk_screen {
            return GuardDecision::Allow;
        }
        // danger 档 = 用户显式信任，屏不生效（与 HighRiskGuard 的档位语义一致）。
        if ctx.sandbox.allows_high_risk() {
            return GuardDecision::Allow;
        }
        // 筛 shell 工具与 run_tests 的自定义命令（同走沙箱化 spawn，红队 P2-7：原注释与实现
        // 矛盾——run_tests 是独立工具名、键为 command）；plugin command 的命令由清单作者
        // + Always 审批把关，不在本屏范围。
        let cmd: &str = match ctx.spec.name.as_str() {
            "shell" => match ctx.call.input.get("cmd").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return GuardDecision::Allow, // 缺参由工具自身报错，守卫不越权
            },
            "run_tests" => match ctx.call.input.get("command").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return GuardDecision::Allow,
            },
            _ => return GuardDecision::Allow,
        };
        match scan(cmd) {
            Some(hit) => GuardDecision::NeedApproval {
                reason: format!(
                    "命令级风险屏命中（{hit}）：该命令涉及破坏性操作，需人工确认。\
                     注：本屏为审批策略层，非 OS 隔离（可被编码/命令替换绕过——真正围栏在进程沙箱）"
                ),
            },
            None => GuardDecision::Allow,
        }
    }
}

/// 纯函数扫描（单测直测；命中返回模式名）。
pub fn scan(cmd: &str) -> Option<&'static str> {
    use std::sync::OnceLock;
    use regex::Regex;
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let pats = PATTERNS.get_or_init(|| {
        // 平台双口味（POSIX + PowerShell + cmd）；正则长度锚定防误匹配（如 format-xxx 变量名）。
        vec![
            (r"(?i)\brm\s+(-[a-z]*r[a-z]*f|-[a-z]*f[a-z]*r)\b", "递归强删 rm -rf"),
            (r"(?i)\bremove-item\b[^|;&\n]*-recurse\b", "递归删除 Remove-Item -Recurse"),
            (r"(?i)\brd\s+/s\b|\bdel\s+(/s|/q\s+/s)", "递归删除 rd/del /s"),
            (r"(?i)\bformat(\.com|\s+[a-z]:)", "磁盘格式化 format"),
            (r"(?i)\bdiskpart\b", "磁盘分区 diskpart"),
            (r"(?i)\breg(\.exe)?\s+(add|delete|import)\b", "注册表写 reg add/delete"),
            (r"(?i)\bregedit(\.exe)?\s+/s\b", "注册表导入 regedit /s"),
            (r"(?i)\b(sc(\.exe)?|net(\.exe)?)\s+(start|stop|delete)\b", "服务启停/删除"),
            (r"(?i)\b(stop|start|restart|remove)-service\b", "服务启停 PowerShell"),
            (r"(?i)\bshutdown\b|\breboot\b|\bhalt\b", "关机/重启"),
            (r"(?i)\bsudo\b|\bsu\s+root\b", "提权 sudo"),
            (r"(?i)\bmkfs(\.\w+)?\b", "文件系统重建 mkfs"),
            (r"(?i)\bdd\s+if=.*of=/dev/", "裸写块设备 dd"),
            (r"(?i)\bstart-process\b[^|;&\n]*-verb\s+runas", "UAC 提权 Start-Process -Verb RunAs"),
            (r"(?i)\btaskkill\s+(/f\b|/im\b)", "强杀进程 taskkill /f /im"),
            (r"(?i)\bset-executionpolicy\b", "PowerShell 执行策略放宽"),
        ]
        .into_iter()
        .map(|(p, name)| (Regex::new(p).expect("cmd risk 正则编译"), name))
        .collect()
    });
    pats.iter().find(|(re, _)| re.is_match(cmd)).map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_and_misses() {
        assert_eq!(scan("rm -rf /tmp/x"), Some("递归强删 rm -rf"));
        assert_eq!(scan("rm -fr /"), Some("递归强删 rm -rf"));
        assert!(scan("rm file.txt").is_none(), "普通 rm 不误报");
        assert_eq!(scan("Remove-Item -Recurse ./node_modules"), Some("递归删除 Remove-Item -Recurse"));
        assert!(scan("Remove-Item ./x.txt").is_none(), "无 -Recurse 不报");
        assert_eq!(scan("reg add HKCU\\Software\\x /v y"), Some("注册表写 reg add/delete"));
        assert!(scan("git add .").is_none(), "git add 不误报");
        assert!(scan("cargo build --release").is_none());
        assert!(scan("npm install").is_none());
        assert!(scan("echo hello").is_none());
        assert_eq!(scan("sudo apt install x"), Some("提权 sudo"));
        assert!(scan("tasklist /FI \"PID eq 1\"").is_none(), "tasklist 只读不报");
        assert_eq!(scan("taskkill /F /IM notepad.exe"), Some("强杀进程 taskkill /f /im"));
        // 漏报面（红队 B-14 清单——如实断言，钉住 advisory 定位）：
        // 命令替换里**明文**出现破坏串时会被字面命中（比文档口径强）；真正的绕过是编码载荷。
        assert!(scan("echo cm0gLXJmIC8= | base64 -d | sh").is_none(), "base64 管道绕过（advisory 定位如实）");
        assert_eq!(scan("echo $(rm -rf /) "), Some("递归强删 rm -rf"), "明文破坏串字面命中");
        assert!(scan("bash ./workdir/payload.sh").is_none(), "脚本内嵌恶意命令绕过（advisory 定位如实）");
    }
}
