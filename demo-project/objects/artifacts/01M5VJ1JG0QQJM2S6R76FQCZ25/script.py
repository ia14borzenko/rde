"""График: читает out/stats.json и рисует гистограмму средних/разбросов (SVG без зависимостей)."""
import json
from pathlib import Path

st = json.loads(Path("inputs/stats.json").read_text(encoding="utf-8"))["stats"]
# Ряды: среднее и стандартное отклонение для каждой величины (x1, x2).
series = []
for key in sorted(st):
    series.append((f"{key}: среднее", st[key]["mean"], "#4f8cff"))
    series.append((f"{key}: σ", st[key]["stdev"], "#ff9f43"))

w, h = 640, 360
left, right, top, bottom = 70, 30, 50, 70
plot_w, plot_h = w - left - right, h - top - bottom
lo = min(v for _, v, _ in series)
hi = max(v for _, v, _ in series)
span = (hi - lo) or 1.0
step = plot_w / len(series)
bars = []
for i, (label, value, color) in enumerate(series):
    bh = max(2.0, (value - lo) / span * (plot_h - 20))
    x = left + i * step + step * 0.18
    bw = step * 0.64
    y = top + plot_h - bh
    bars.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{bw:.1f}" height="{bh:.1f}" fill="{color}"/>')
    bars.append(f'<text x="{x + bw/2:.1f}" y="{y - 6:.1f}" font-family="DejaVu Sans" font-size="11" text-anchor="middle" fill="#222">{value:.3f}</text>')
    bars.append(f'<text x="{x + bw/2:.1f}" y="{top + plot_h + 18:.1f}" font-family="DejaVu Sans" font-size="11" text-anchor="middle" fill="#333">{label}</text>')
axes = (
    f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_h}" stroke="#333" stroke-width="1.2"/>'
    f'<line x1="{left}" y1="{top + plot_h}" x2="{left + plot_w}" y2="{top + plot_h}" stroke="#333" stroke-width="1.2"/>'
    f'<text x="{left}" y="26" font-family="DejaVu Sans" font-size="15" font-weight="bold" fill="#111">Статистики реализаций: средние и разбросы</text>'
    f'<text x="{left - 8}" y="{top - 8}" font-family="DejaVu Sans" font-size="10" text-anchor="end" fill="#555">{hi:.3f}</text>'
    f'<text x="{left - 8}" y="{top + plot_h}" font-family="DejaVu Sans" font-size="10" text-anchor="end" fill="#555">{lo:.3f}</text>'
)
svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">'
    f'<rect width="{w}" height="{h}" fill="white"/>{axes}{"".join(bars)}</svg>'
)
Path("out").mkdir(exist_ok=True)
Path("out/plot.svg").write_text(svg, encoding="utf-8")
print("wrote out/plot.svg")
