//! Token 粗估与上下文分桶（压缩方案 §4.1.4）。
//!
//! 用途：① 无真实 usage 网关的用量显示退化；② 自动压缩触发判定；③ 明细卡「分类占比」。
//! 口径：CJK 字符 ×0.6 + 其它字符 ÷4（向上取整）。opencode/codex 的估算器是纯 len÷4，
//! 对中文低估约 2.4 倍——CJK 系数是为中文会话纠偏，非多余复杂度。±20% 级粗估，
//! 触发线另留余量吸收（§7.3）。

use crate::model::ModelContext;

/// 粗估一段文本的 token 数（CJK×0.6 + 其余÷4，均向上取整；纯整数运算 ceil(a/b)=(a+b-1)/b）。
pub fn est_tokens(s: &str) -> u64 {
    let mut cjk: u64 = 0;
    let mut other: u64 = 0;
    for c in s.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    let cjk_tokens = (cjk * 3).div_ceil(5); // ceil(0.6n)
    let other_tokens = other.div_ceil(4);
    cjk_tokens + other_tokens
}

/// CJK 表意字符（汉字 + 全角标点 + 假名/谚文一并按高密度计）。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // CJK 标点
        | 0x3040..=0x30FF // 假名
        | 0x3400..=0x4DBF // 扩展 A
        | 0x4E00..=0x9FFF // 基本区
        | 0xF900..=0xFAFF // 兼容表意
        | 0xFF00..=0xFFEF // 全角形式
        | 0x20000..=0x2A6DF // 扩展 B
    )
}

/// 上下文分类分桶（明细卡「分类占比」的直接数据源）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ContextBreakdown {
    /// 用户/助手文本与工具调用参数。
    pub messages: u64,
    /// 工具输出。
    pub tool_results: u64,
    /// 工具清单（schema JSON）。
    pub tool_defs: u64,
    /// 系统提示词。
    pub system: u64,
    /// 余量（当前恒 0，留扩展位）。
    pub other: u64,
    /// 合计。
    pub total: u64,
}

/// 对投影结果按明细卡分类分桶估算。
pub fn est_context(ctx: &ModelContext) -> ContextBreakdown {
    let mut b = ContextBreakdown::default();
    if let Some(sys) = &ctx.system {
        b.system = est_tokens(sys);
    }
    for m in &ctx.messages {
        match m {
            crate::model::ModelMessage::User { text } => b.messages += est_tokens(text),
            crate::model::ModelMessage::Assistant { text, tool_calls } => {
                if let Some(t) = text {
                    b.messages += est_tokens(t);
                }
                for c in tool_calls {
                    b.messages += est_tokens(&c.input.to_string());
                }
            }
            crate::model::ModelMessage::Tool { output, .. } => {
                b.tool_results += est_tokens(&output.to_string());
            }
        }
    }
    for t in &ctx.tools {
        b.tool_defs += est_tokens(&t.input_schema.to_string());
        b.tool_defs += est_tokens(&t.description);
    }
    b.total = b.messages + b.tool_results + b.tool_defs + b.system + b.other;
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelMessage, ModelContext};
    use serde_json::json;

    #[test]
    fn cjk_ratio_applied() {
        // 10 个汉字 ≈ 6 token（0.6 系数）
        assert_eq!(est_tokens("零一二三四五六七八九"), 6);
        // 40 个 ASCII ≈ 10 token（÷4）
        assert_eq!(est_tokens(&"a".repeat(40)), 10);
    }

    #[test]
    fn mixed_and_empty() {
        assert_eq!(est_tokens(""), 0);
        // 6 汉字(ceil 3.6=4) + 8 ascii(2) = 6
        assert_eq!(est_tokens("你好世界再见abcdefgh"), 6);
    }

    #[test]
    fn buckets_by_message_kind() {
        let ctx = ModelContext {
            system: Some("系统提示".into()),
            messages: vec![
                ModelMessage::User { text: "用户问题".into() },
                ModelMessage::Tool { call_id: "c1".into(), output: json!("工具输出内容") },
            ],
            tools: vec![],
        };
        let b = est_context(&ctx);
        assert!(b.system > 0 && b.messages > 0 && b.tool_results > 0);
        assert_eq!(b.total, b.system + b.messages + b.tool_results + b.other);
    }
}
