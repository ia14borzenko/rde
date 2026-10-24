"""Статистики реализаций: читает CSV, считает среднее/дисперсию, пишет out/stats.json."""
import csv, json, statistics
from pathlib import Path

src = Path("inputs/experiment.csv")
rows = list(csv.DictReader(src.open(encoding="utf-8")))
cols = {k: [float(r[k]) for r in rows] for k in ("x1", "x2")}
out = {
    "n": len(rows),
    "stats": {
        k: {"mean": statistics.fmean(v), "stdev": statistics.pstdev(v), "min": min(v), "max": max(v)}
        for k, v in cols.items()
    },
}
Path("out").mkdir(exist_ok=True)
Path("out/stats.json").write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
print("wrote out/stats.json")
