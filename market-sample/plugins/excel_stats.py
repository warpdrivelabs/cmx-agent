#!/usr/bin/env python3
"""excel_stats.py —— 演示插件 excel-stats 的被包装脚本。

「stdin 参数 → stdout 结果」约定的最小实现：
- 参数走 argparse（清单 args 里 --file {file} 占位替换进来）；
- stdout 输出 JSON（= 模型看到的工具结果）；报错也打到 stdout（非零退出码 + JSON error）。
只支持 CSV（内置 csv 模块，零依赖）；.xlsx 打印提示改存 CSV。
"""
import csv
import json
import sys

def main():
    args = sys.argv[1:]
    if "--file" not in args or args.index("--file") + 1 >= len(args):
        print(json.dumps({"error": "缺少 --file 参数"}))
        return 1
    path = args[args.index("--file") + 1]
    if path.lower().endswith(".xlsx"):
        print(json.dumps({"error": "暂只支持 CSV；请把 Excel 另存为 CSV 后重试"}))
        return 1
    try:
        with open(path, newline="", encoding="utf-8-sig") as f:
            rows = list(csv.reader(f))
    except OSError as e:
        print(json.dumps({"error": f"读取失败：{e}"}))
        return 1
    if not rows:
        print(json.dumps({"error": "文件为空"}))
        return 1
    header, data = rows[0], rows[1:]
    cols = []
    for i, name in enumerate(header):
        vals = [r[i] for r in data if i < len(r)]
        empty = sum(1 for v in vals if not v.strip())
        distinct = len({v for v in vals if v.strip()})
        cols.append({
            "column": name,
            "rows": len(vals),
            "empty_rate": round(empty / len(vals), 4) if vals else 0,
            "distinct": distinct,
        })
    print(json.dumps({"file": path, "data_rows": len(data), "columns": cols}, ensure_ascii=False))
    return 0

if __name__ == "__main__":
    sys.exit(main())
