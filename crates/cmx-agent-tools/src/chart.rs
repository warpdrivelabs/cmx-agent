//! `chart` —— 把数据渲染成图表（手写 SVG，无额外依赖）：bar 柱状 / line 折线 / pie 饼图。
//! 写一份 .svg 到工作区（save_as），并把 SVG 内联返回给前端展示。需 workspace-write（写文件）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::sandbox;

pub struct ChartTool;

/// 分类调色板（对齐 dataviz 规范的高区分度顺序）。
const PALETTE: &[&str] = &[
    "#3987e5", "#1fb182", "#e5b95f", "#d55181", "#9085e9", "#eb6834", "#36cfc9", "#22c55e",
];
const INK: &str = "#0b0f16";
const GRID: &str = "#d6d5ce";
const MUTED: &str = "#7c8a99";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[async_trait]
impl Tool for ChartTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "chart",
            "把数据渲染成图表 SVG（bar 柱状 / line 折线 / pie 饼图）。给 labels + series；可 save_as 存到工作区。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "type": { "type": "string", "enum": ["bar","line","pie"] },
                "title": { "type": "string" },
                "labels": { "type": "array", "items": {"type":"string"}, "description": "X 轴/分类标签" },
                "series": {
                    "type": "array",
                    "items": { "type":"object", "properties": {
                        "name": {"type":"string"}, "values": {"type":"array","items":{"type":"number"}}
                    }, "required": ["values"] },
                    "description": "一或多个数据系列（pie 只取第一个）"
                },
                "save_as": { "type": "string", "description": "可选：存到工作区的 .svg 文件名" }
            },
            "required": ["type","labels","series"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:write".into()), idempotent: false, ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let kind = input.get("type").and_then(|v| v.as_str()).unwrap_or("bar");
        let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let labels: Vec<String> = input
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        let series: Vec<Series> = input
            .get("series")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|s| Series {
                        name: s.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                        values: s
                            .get("values")
                            .and_then(|vs| vs.as_array())
                            .map(|vs| vs.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        if labels.is_empty() || series.is_empty() {
            return Ok(ToolResult::err("chart: labels 与 series 不能为空"));
        }

        let svg = match kind {
            "pie" => render_pie(title, &labels, &series[0]),
            "line" => render_xy(title, &labels, &series, true),
            _ => render_xy(title, &labels, &series, false),
        };

        // 可选写文件
        let mut saved: Option<String> = None;
        if let Some(name) = input.get("save_as").and_then(|v| v.as_str()) {
            if !ctx.sandbox.allows_write() {
                return Ok(ToolResult::err("chart: save_as 需 workspace-write 沙箱"));
            }
            match sandbox::resolve(name, ctx) {
                Ok(p) => match std::fs::write(&p, svg.as_bytes()) {
                    Ok(()) => saved = Some(p.display().to_string()),
                    Err(e) => return Ok(ToolResult::err(format!("chart: 写文件失败 {e}"))),
                },
                Err(e) => return Ok(ToolResult::err(format!("chart: {e}"))),
            }
        }

        Ok(ToolResult::ok(json!({
            "type": kind,
            "svg": svg,
            "saved": saved,
        })))
    }
}

struct Series {
    name: String,
    values: Vec<f64>,
}

fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let n = v / mag;
    let step = if n <= 1.0 { 1.0 } else if n <= 2.0 { 2.0 } else if n <= 5.0 { 5.0 } else { 10.0 };
    step * mag
}

/// 柱状图（grouped）/折线图共用 X-Y 画布。
fn render_xy(title: &str, labels: &[String], series: &[Series], line: bool) -> String {
    let w = 720.0;
    let h = 420.0;
    let (ml, mr, mt, mb) = (56.0, 24.0, if title.is_empty() { 24.0 } else { 48.0 }, 64.0);
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    let maxv = series
        .iter()
        .flat_map(|s| s.values.iter().cloned())
        .fold(0.0f64, f64::max);
    let ymax = nice_max(maxv);
    let n = labels.len().max(1);

    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="-apple-system,'PingFang SC',sans-serif">
<rect width="{w}" height="{h}" fill='#ffffff'/>"#
    );
    if !title.is_empty() {
        s += &format!(
            r#"<text x="{}" y="30" font-size="16" font-weight="700" fill="{INK}">{}</text>"#,
            ml, esc(title)
        );
    }
    // Y 轴网格 + 刻度（5 格）
    for i in 0..=5 {
        let yv = ymax * (i as f64) / 5.0;
        let y = mt + ph - ph * (i as f64) / 5.0;
        s += &format!(
            r#"<line x1="{ml}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="{GRID}" stroke-width="1"/>"#,
            ml + pw
        );
        s += &format!(
            r#"<text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}" text-anchor="end">{}</text>"#,
            ml - 8.0,
            y + 4.0,
            fmt_num(yv)
        );
    }
    // X 轴标签
    let slot = pw / n as f64;
    for (i, lab) in labels.iter().enumerate() {
        let x = ml + slot * (i as f64 + 0.5);
        s += &format!(
            r#"<text x="{x:.1}" y="{:.1}" font-size="11" fill="{MUTED}" text-anchor="middle">{}</text>"#,
            mt + ph + 20.0,
            esc(lab)
        );
    }

    if line {
        // 折线：每个系列一条折线 + 点
        for (si, ser) in series.iter().enumerate() {
            let col = PALETTE[si % PALETTE.len()];
            let mut pts = String::new();
            for (i, v) in ser.values.iter().enumerate() {
                let x = ml + slot * (i as f64 + 0.5);
                let y = mt + ph - ph * (v / ymax);
                pts += &format!("{x:.1},{y:.1} ");
            }
            s += &format!(r#"<polyline points="{pts}" fill="none" stroke="{col}" stroke-width="2.5"/>"#);
            for (i, v) in ser.values.iter().enumerate() {
                let x = ml + slot * (i as f64 + 0.5);
                let y = mt + ph - ph * (v / ymax);
                s += &format!(r#"<circle cx="{x:.1}" cy="{y:.1}" r="3.5" fill="{col}"/>"#);
            }
        }
    } else {
        // 柱状：每个标签一组，组内各系列并排
        let ns = series.len().max(1);
        let group_w = slot * 0.7;
        let bar_w = group_w / ns as f64;
        for (i, _) in labels.iter().enumerate() {
            let gx = ml + slot * i as f64 + (slot - group_w) / 2.0;
            for (si, ser) in series.iter().enumerate() {
                let v = ser.values.get(i).cloned().unwrap_or(0.0);
                let bh = ph * (v / ymax);
                let x = gx + bar_w * si as f64;
                let y = mt + ph - bh;
                let col = PALETTE[si % PALETTE.len()];
                s += &format!(
                    r#"<rect x="{x:.1}" y="{y:.1}" width="{:.1}" height="{bh:.1}" rx="2" fill="{col}"/>"#,
                    bar_w * 0.86
                );
            }
        }
    }
    // 图例（多系列时）
    if series.len() > 1 || !series[0].name.is_empty() {
        let mut lx = ml;
        let ly = h - 18.0;
        for (si, ser) in series.iter().enumerate() {
            let col = PALETTE[si % PALETTE.len()];
            let name = if ser.name.is_empty() { format!("系列{}", si + 1) } else { ser.name.clone() };
            s += &format!(r#"<rect x="{lx:.1}" y="{:.1}" width="12" height="12" rx="3" fill="{col}"/>"#, ly - 10.0);
            s += &format!(
                r#"<text x="{:.1}" y="{ly:.1}" font-size="11.5" fill="{INK}">{}</text>"#,
                lx + 17.0,
                esc(&name)
            );
            lx += 30.0 + name.chars().count() as f64 * 9.0;
        }
    }
    s += "</svg>";
    s
}

fn render_pie(title: &str, labels: &[String], ser: &Series) -> String {
    let w = 640.0;
    let h = 420.0;
    let cx = 210.0;
    let cy = h / 2.0 + if title.is_empty() { 0.0 } else { 10.0 };
    let r = 150.0;
    let total: f64 = ser.values.iter().sum();
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="-apple-system,'PingFang SC',sans-serif">
<rect width="{w}" height="{h}" fill='#ffffff'/>"#
    );
    if !title.is_empty() {
        s += &format!(r#"<text x="28" y="30" font-size="16" font-weight="700" fill="{INK}">{}</text>"#, esc(title));
    }
    if total <= 0.0 {
        s += &format!(r#"<text x="{cx}" y="{cy}" font-size="13" fill="{MUTED}" text-anchor="middle">无数据</text></svg>"#);
        return s;
    }
    let mut ang = -std::f64::consts::FRAC_PI_2; // 从顶部开始
    for (i, v) in ser.values.iter().enumerate() {
        let frac = v / total;
        let a2 = ang + frac * std::f64::consts::TAU;
        let (x1, y1) = (cx + r * ang.cos(), cy + r * ang.sin());
        let (x2, y2) = (cx + r * a2.cos(), cy + r * a2.sin());
        let large = if frac > 0.5 { 1 } else { 0 };
        let col = PALETTE[i % PALETTE.len()];
        s += &format!(
            r#"<path d="M{cx:.1},{cy:.1} L{x1:.1},{y1:.1} A{r},{r} 0 {large},1 {x2:.1},{y2:.1} Z" fill="{col}"/>"#
        );
        ang = a2;
    }
    // 图例（右侧）
    let mut ly = cy - r + 10.0;
    for (i, lab) in labels.iter().enumerate() {
        let col = PALETTE[i % PALETTE.len()];
        let v = ser.values.get(i).cloned().unwrap_or(0.0);
        let pct = v / total * 100.0;
        s += &format!(r#"<rect x="420" y="{:.1}" width="13" height="13" rx="3" fill="{col}"/>"#, ly - 10.0);
        s += &format!(
            r#"<text x="440" y="{ly:.1}" font-size="12.5" fill="{INK}">{} · {:.1}%</text>"#,
            esc(lab),
            pct
        );
        ly += 26.0;
    }
    s += "</svg>";
    s
}

fn fmt_num(v: f64) -> String {
    // 大数与整数不带小数位；其余保留一位小数。
    if v.abs() >= 1000.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn ctx_roots() -> Vec<PathBuf> {
        vec![crate::testutil::unique_dir("cmx-chart")]
    }

    #[tokio::test]
    async fn bar_chart_returns_svg() {
        let roots = ctx_roots();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ChartTool
            .invoke(json!({"type":"bar","title":"销量","labels":["一月","二月","三月"],
                "series":[{"name":"A","values":[10,20,15]}]}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        let svg = r.output["svg"].as_str().unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<rect")); // 有柱
        std::fs::remove_dir_all(&roots[0]).ok();
    }

    #[tokio::test]
    async fn pie_and_save() {
        let roots = ctx_roots();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ChartTool
            .invoke(json!({"type":"pie","labels":["北","南"],"series":[{"values":[60,40]}],"save_as":"p.svg"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert!(r.output["svg"].as_str().unwrap().contains("<path"));
        assert!(r.output["saved"].as_str().unwrap().ends_with("p.svg"));
        assert!(roots[0].join("p.svg").exists());
        std::fs::remove_dir_all(&roots[0]).ok();
    }

    #[tokio::test]
    async fn empty_errors() {
        let roots = ctx_roots();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ChartTool.invoke(json!({"type":"bar","labels":[],"series":[]}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&roots[0]).ok();
    }
}
