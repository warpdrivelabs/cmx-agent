//! Excel：calamine 读（xlsx/xls/ods）+ rust_xlsxwriter 写。返回/接收结构化行数据。

use serde_json::{Value, json};

/// 读一个 Excel 工作簿，返回每个 sheet 的行数据（截断到 max_rows/max_cols）。
pub fn read_excel(path: &std::path::Path, max_rows: usize, max_cols: usize) -> Result<Value, String> {
    use calamine::{Reader, open_workbook_auto};
    let mut wb = open_workbook_auto(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut sheets = Vec::new();
    let names = wb.sheet_names().to_vec();
    for name in names {
        let range = match wb.worksheet_range(&name) {
            Ok(r) => r,
            Err(e) => {
                sheets.push(json!({ "name": name, "error": e.to_string() }));
                continue;
            }
        };
        let (nrows, ncols) = range.get_size();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (ri, row) in range.rows().enumerate() {
            if ri >= max_rows {
                break;
            }
            let cells: Vec<String> = row.iter().take(max_cols).map(cell_to_string).collect();
            rows.push(cells);
        }
        sheets.push(json!({
            "name": name,
            "rows_total": nrows,
            "cols_total": ncols,
            "truncated": nrows > max_rows || ncols > max_cols,
            "data": rows,
        }));
    }
    Ok(json!({ "sheets": sheets }))
}

fn cell_to_string(c: &calamine::Data) -> String {
    use calamine::Data;
    match c {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            if f.fract() == 0.0 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{e:?}"),
    }
}

/// 写一个 xlsx：`sheets` = [{name, rows: [[cell,...],...]}]。数值型字符串写成数字。
pub fn write_excel(path: &std::path::Path, sheets: &Value) -> Result<(), String> {
    use rust_xlsxwriter::Workbook;
    let mut wb = Workbook::new();
    let arr = sheets.as_array().ok_or("sheets 应为数组")?;
    if arr.is_empty() {
        return Err("sheets 不能为空".into());
    }
    for (si, sh) in arr.iter().enumerate() {
        let ws = wb.add_worksheet();
        if let Some(name) = sh.get("name").and_then(|v| v.as_str()) {
            let _ = ws.set_name(name);
        } else {
            let _ = ws.set_name(format!("Sheet{}", si + 1));
        }
        let rows = sh.get("rows").and_then(|v| v.as_array()).ok_or("sheet.rows 应为二维数组")?;
        for (r, row) in rows.iter().enumerate() {
            let cells = row.as_array().ok_or("每行应为数组")?;
            for (c, cell) in cells.iter().enumerate() {
                let (rr, cc) = (r as u32, c as u16);
                match cell {
                    Value::Number(n) => {
                        let _ = ws.write_number(rr, cc, n.as_f64().unwrap_or(0.0));
                    }
                    Value::Bool(b) => {
                        let _ = ws.write_boolean(rr, cc, *b);
                    }
                    Value::String(s) => {
                        // 纯数字字符串也写成数字，便于后续计算
                        if let Ok(f) = s.parse::<f64>() {
                            let _ = ws.write_number(rr, cc, f);
                        } else {
                            let _ = ws.write_string(rr, cc, s);
                        }
                    }
                    Value::Null => {}
                    other => {
                        let _ = ws.write_string(rr, cc, other.to_string());
                    }
                }
            }
        }
    }
    wb.save(path).map_err(|e| format!("保存失败: {e}"))?;
    Ok(())
}
