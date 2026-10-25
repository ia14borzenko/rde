"""Линейный график реализаций: первые N точек x1/x2 из CSV (SVG без зависимостей)."""
import csv, os
from pathlib import Path

points = int(os.environ.get("RDE_POINTS", "80"))
rows = list(csv.DictReader(Path("inputs/experiment.csv").open(encoding="utf-8")))[:points]
x1 = [float(r["x1"]) for r in rows]
x2 = [float(r["x2"]) for r in rows]

w, h = 640, 360
left, right, top, bottom = 64, 24, 46, 58
plot_w, plot_h = w - left - right, h - top - bottom
lo = min(min(x1), min(x2))
hi = max(max(x1), max(x2))
span = (hi - lo) or 1.0


def line(series, color):
    step = plot_w / max(1, len(series) - 1)
    pts = []
    for i, value in enumerate(series):
        x = left + i * step
        y = top + plot_h - (value - lo) / span * plot_h
        pts.append(f"{x:.1f},{y:.1f}")
    return f'<polyline points="{" ".join(pts)}" fill="none" stroke="{color}" stroke-width="1.6"/>'


svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">'
    f'<rect width="{w}" height="{h}" fill="white"/>'
    f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_h}" stroke="#333" stroke-width="1.2"/>'
    f'<line x1="{left}" y1="{top + plot_h}" x2="{left + plot_w}" y2="{top + plot_h}" stroke="#333" stroke-width="1.2"/>'
    f'<text x="{left}" y="26" font-family="DejaVu Sans" font-size="15" font-weight="bold" fill="#111">Реализации случайного процесса (первые {len(rows)} точек)</text>'
    f'<text x="{left}" y="{top + plot_h + 40}" font-family="DejaVu Sans" font-size="11" fill="#333">— x1 (синий)   — x2 (оранжевый)</text>'
    f'<text x="{left - 8}" y="{top + 4}" font-family="DejaVu Sans" font-size="10" text-anchor="end" fill="#555">{hi:.2f}</text>'
    f'<text x="{left - 8}" y="{top + plot_h}" font-family="DejaVu Sans" font-size="10" text-anchor="end" fill="#555">{lo:.2f}</text>'
    f'{line(x1, "#4f8cff")}{line(x2, "#ff9f43")}</svg>'
)
Path("out").mkdir(exist_ok=True)
Path("out/line.svg").write_text(svg, encoding="utf-8")
print("wrote out/line.svg")
