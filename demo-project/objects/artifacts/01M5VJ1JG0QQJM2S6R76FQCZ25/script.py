"""График: читает out/stats.json, рисует простой SVG без внешних зависимостей."""
import json
from pathlib import Path

st = json.loads(Path("inputs/stats.json").read_text(encoding="utf-8"))["stats"]
w, h, pad = 480, 260, 28
vals = [(k, v["mean"]) for k, v in sorted(st.items())]
lo = min(v for _, v in vals); hi = max(v for _, v in vals)
span = (hi - lo) or 1.0
bars = []
for i, (k, v) in enumerate(vals):
    bh = (v - lo) / span * (h - 2 * pad)
    x = pad + i * 120
    bars.append(f'<rect x="{x}" y="{h - pad - bh:.1f}" width="60" height="{bh:.1f}" fill="#4f8cff"/>')
    bars.append(f'<text x="{x + 30}" y="{h - 8}" font-size="11" text-anchor="middle">{k}</text>')
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">'
       f'<rect width="{w}" height="{h}" fill="white"/>' + "".join(bars) + '</svg>')
Path("out").mkdir(exist_ok=True)
Path("out/plot.svg").write_text(svg, encoding="utf-8")
print("wrote out/plot.svg")
