"""Plot measured ordinary wheel glides, normalized to observed native input."""
from pathlib import Path
import json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

root = Path(__file__).resolve().parent
fig, axes = plt.subplots(1, 2, figsize=(11, 4.4))
for filename, label, color in [
    ('default-glide.json', 'CodeMux default glide', '#397d58'),
    ('electron-ticks.json', 'Electron comparison shell', '#347d9d'),
]:
    data = json.loads((root/'glide-results'/filename).read_text())
    frames, wheels = data['frames'], data['wheels']
    for ax, first, last, until in [(axes[0], 0, 0, 260), (axes[1], 12, 16, 850)]:
        start = wheels[first][0]
        base = next(p for t, p in reversed(frames) if t <= start)
        total = sum(w[1] for w in wheels[first:last+1])
        points = [(t-start, (p-base)/total*100) for t, p in frames if start <= t <= start+until]
        ax.plot(*zip(*points), label=label, color=color, linewidth=2)
        ax.set_xlim(0, until)
        ax.set_ylim(-1, 105)
        ax.set_xlabel('Time after first wheel tick (ms)')
        ax.set_ylabel('Requested distance covered (%)')
        ax.spines[['top', 'right']].set_visible(False)
        ax.grid(alpha=.2)
axes[0].set_title('One ~25 px tick: gradual movement')
axes[1].set_title('Five ticks 150 ms apart: continuous glide')
axes[0].legend(frameon=False, loc='upper left')
fig.suptitle('CodeMux wheel-glide candidate compared with Chromium', fontsize=14)
fig.text(.07, .025, 'Native injected input • observed callback positions • not a physical mouse or scanout recording', fontsize=9)
fig.tight_layout(rect=[0, .065, 1, .94])
fig.savefig(root/'glide-chart.png', dpi=180)
fig.savefig(root/'glide-chart.svg')
