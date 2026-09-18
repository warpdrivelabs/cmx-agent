//! `data_describe` —— 读工作区 CSV/TSV，给出结构化统计（行数、每列类型 + 数值列 min/max/mean/sum、
//! 文本列 distinct/top 值），供办公助手快速摸清数据。只读。

use std::collections::HashMap;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::paths;

pub struct DataDescribeTool;

#[async_trait]
impl Tool for DataDescribeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "data_describe",
            "分析 CSV/TSV 数据：行数、每列类型与统计（数值列 min/max/mean/sum，文本列 distinct/top 值）+ 采样",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "CSV/TSV 路径（相对工作目录或绝对路径）" },
                "delimiter": { "type": "string", "description": "分隔符，默认 ','（TSV 用 '\\t'）" },
                "sample_rows": { "type": "integer", "default": 5, "description": "返回前若干行采样" }
            },
            "required": ["path"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:read".into()), idempotent: true, ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("data_describe: 'path' is required"));
        };
        let abs = match paths::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("data_describe: {e}"))),
        };
        let delim = input
            .get("delimiter")
            .and_then(|v| v.as_str())
            .and_then(|s| s.bytes().next())
            .unwrap_or(b',');
        let sample_rows = input.get("sample_rows").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(format!("data_describe: 读取失败 {e}"))),
        };
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delim)
            .flexible(true)
            .from_reader(content.as_bytes());
        let headers: Vec<String> = match rdr.headers() {
            Ok(h) => h.iter().map(|s| s.to_string()).collect(),
            Err(e) => return Ok(ToolResult::err(format!("data_describe: 解析表头失败 {e}"))),
        };
        if headers.is_empty() {
            return Ok(ToolResult::err("data_describe: 空文件或无表头"));
        }

        let ncol = headers.len();
        let mut cols: Vec<ColStat> = (0..ncol).map(|_| ColStat::default()).collect();
        let mut sample: Vec<Vec<String>> = Vec::new();
        let mut nrows = 0u64;

        for rec in rdr.records().flatten() {
            nrows += 1;
            if sample.len() < sample_rows {
                sample.push(rec.iter().map(|s| s.to_string()).collect());
            }
            for (i, field) in rec.iter().enumerate() {
                if i < ncol {
                    cols[i].observe(field);
                }
            }
        }

        let columns: Vec<Value> = headers
            .iter()
            .zip(cols.iter())
            .map(|(name, c)| c.to_json(name))
            .collect();

        Ok(ToolResult::ok(json!({
            "rows": nrows,
            "columns": columns,
            "sample": json!({ "headers": headers, "rows": sample }),
        })))
    }
}

#[derive(Default)]
struct ColStat {
    count: u64,
    empty: u64,
    numeric_count: u64,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
    distinct: HashMap<String, u64>,
}

impl ColStat {
    fn observe(&mut self, raw: &str) {
        let v = raw.trim();
        self.count += 1;
        if v.is_empty() {
            self.empty += 1;
            return;
        }
        if let Ok(n) = v.parse::<f64>() {
            self.numeric_count += 1;
            self.sum += n;
            self.min = Some(self.min.map_or(n, |m| m.min(n)));
            self.max = Some(self.max.map_or(n, |m| m.max(n)));
        }
        // distinct（仅保留有限个，避免爆内存）
        if self.distinct.len() < 10_000 {
            *self.distinct.entry(v.to_string()).or_insert(0) += 1;
        }
    }

    fn to_json(&self, name: &str) -> Value {
        let non_empty = self.count - self.empty;
        // 判定为数值列：非空值中 ≥80% 可解析为数字
        let is_numeric = non_empty > 0 && self.numeric_count * 100 >= non_empty * 80;
        if is_numeric {
            let mean = if self.numeric_count > 0 { self.sum / self.numeric_count as f64 } else { 0.0 };
            json!({
                "name": name, "type": "numeric", "count": self.count, "empty": self.empty,
                "min": self.min, "max": self.max, "mean": (mean*1000.0).round()/1000.0, "sum": self.sum,
            })
        } else {
            // top 3 值
            let mut pairs: Vec<(&String, &u64)> = self.distinct.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1));
            let top: Vec<Value> = pairs
                .into_iter()
                .take(3)
                .map(|(k, v)| json!({ "value": k, "count": v }))
                .collect();
            json!({
                "name": name, "type": "text", "count": self.count, "empty": self.empty,
                "distinct": self.distinct.len(), "top": top,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup(csv: &str) -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-data");
        std::fs::write(root.join("d.csv"), csv).unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn describes_numeric_and_text() {
        let (root, roots) = setup("city,pop,region\nBJ,100,north\nSH,90,east\nBJ,80,north\n");
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = DataDescribeTool.invoke(json!({"path":"d.csv"}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["rows"], 3);
        let cols = r.output["columns"].as_array().unwrap();
        // pop 列数值统计
        let pop = cols.iter().find(|c| c["name"]=="pop").unwrap();
        assert_eq!(pop["type"], "numeric");
        assert_eq!(pop["min"], 80.0);
        assert_eq!(pop["max"], 100.0);
        assert_eq!(pop["mean"], 90.0);
        // city 列文本 distinct=2（BJ,SH），top 含 BJ(2)
        let city = cols.iter().find(|c| c["name"]=="city").unwrap();
        assert_eq!(city["type"], "text");
        assert_eq!(city["distinct"], 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let (root, roots) = setup("a\n1\n");
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = DataDescribeTool.invoke(json!({"path":"nope.csv"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
