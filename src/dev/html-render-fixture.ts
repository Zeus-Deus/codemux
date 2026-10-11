/**
 * A synthetic `html_render` page for `?visual=1` in `npm run dev`: the kind
 * of dashboard an agent publishes into a thread. Every number is made up,
 * and every color comes from the injected theme variables, so switching the
 * app theme repaints it live.
 */
export const HTML_RENDER_FIXTURE_TITLE = "Agent activity insights";

export const HTML_RENDER_FIXTURE_HTML = `<!doctype html>
<html>
<head>
<style>
  .tiles { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 1px; background: var(--border); border-radius: var(--radius); overflow: hidden; }
  .tile { background: var(--background); padding: 10px 12px; }
  .tile b { display: block; font-size: 20px; font-weight: 600; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
  .tile span { font-size: 12px; color: var(--muted-foreground); }
  h3 { margin: 24px 0 2px; font-size: 14px; font-weight: 600; }
  .sub { margin: 0 0 10px; font-size: 12px; color: var(--muted-foreground); }
  .legend { display: flex; flex-wrap: wrap; gap: 4px 14px; margin-bottom: 8px; font-size: 12px; color: var(--muted-foreground); }
  .legend i { display: inline-block; width: 8px; height: 8px; border-radius: 2px; margin-right: 6px; }
  svg { display: block; width: 100%; overflow: visible; }
  svg text { fill: var(--muted-foreground); font-size: 10px; font-family: var(--font-sans); }
  .grid line { stroke: var(--border); }
  .tip { position: fixed; pointer-events: none; padding: 6px 8px; border-radius: 6px; font-size: 12px; background: var(--popover); color: var(--popover-foreground); border: 1px solid var(--border); box-shadow: 0 4px 16px rgb(0 0 0 / 0.18); opacity: 0; transition: opacity 80ms; white-space: nowrap; }
  .tip div { display: flex; align-items: center; gap: 6px; font-variant-numeric: tabular-nums; }
  .tip i { width: 8px; height: 8px; border-radius: 2px; }
  .heat { display: grid; grid-template-columns: 28px repeat(24, minmax(0, 1fr)); gap: 2px; font-size: 10px; color: var(--muted-foreground); }
  .heat .cell { height: 16px; border-radius: 3px; }
  .heat .lbl { align-self: center; }
  .scale { display: flex; align-items: center; gap: 2px; margin-top: 8px; font-size: 10px; color: var(--muted-foreground); }
  .scale i { width: 14px; height: 8px; border-radius: 2px; }
</style>
</head>
<body>
<div class="tiles">
  <div class="tile"><b>18.4k</b><span>turns sent</span></div>
  <div class="tile"><b>1,212</b><span>chat sessions</span></div>
  <div class="tile"><b>42s</b><span>median turn</span></div>
  <div class="tile"><b>61%</b><span>turns on Claude</span></div>
</div>

<h3>Codex share doubled after the new sandbox shipped</h3>
<p class="sub">Share of turns per week by provider. 12 weeks.</p>
<div class="legend" id="legend"></div>
<svg id="area" height="190"></svg>

<h3>Peak is Wednesday afternoon</h3>
<p class="sub">Turns by weekday and hour, local time.</p>
<div class="heat" id="heat"></div>
<div class="scale"><span>fewer</span><span id="scale" style="display:flex;gap:2px"></span><span>more</span></div>
<div class="tip" id="tip"></div>

<script>
  const series = [
    { name: "Claude", color: "var(--chart-1)", values: [66, 67, 65, 66, 64, 63, 62, 60, 58, 57, 56, 55] },
    { name: "Codex", color: "var(--chart-2)", values: [12, 12, 13, 13, 14, 15, 18, 22, 24, 25, 26, 27] },
    { name: "OpenCode", color: "var(--chart-3)", values: [10, 10, 11, 11, 12, 12, 11, 10, 10, 10, 10, 10] },
    { name: "Other", color: "var(--chart-4)", values: [12, 11, 11, 10, 10, 10, 9, 8, 8, 8, 8, 8] },
  ];
  const weeks = ["Jul 13", "Jul 20", "Jul 27", "Aug 3", "Aug 10", "Aug 17", "Aug 24", "Aug 31", "Sep 7", "Sep 14", "Sep 21", "Sep 28"];
  const tip = document.getElementById("tip");
  const showTip = (event, html) => {
    tip.innerHTML = html;
    tip.style.opacity = "1";
    const x = Math.min(event.clientX + 12, window.innerWidth - tip.offsetWidth - 4);
    tip.style.left = x + "px";
    tip.style.top = event.clientY + 12 + "px";
  };
  const hideTip = () => { tip.style.opacity = "0"; };

  document.getElementById("legend").innerHTML = series
    .map((s) => '<span><i style="background:' + s.color + '"></i>' + s.name + "</span>")
    .join("");

  const svg = document.getElementById("area");
  const ns = "http://www.w3.org/2000/svg";
  const el = (name, attrs) => {
    const node = document.createElementNS(ns, name);
    for (const key in attrs) node.setAttribute(key, attrs[key]);
    return node;
  };
  function drawArea() {
    svg.textContent = "";
    const width = svg.clientWidth, height = 190, left = 30, bottom = 18;
    const plotW = width - left, plotH = height - bottom;
    const x = (i) => left + (i / (weeks.length - 1)) * plotW;
    const y = (v) => plotH - (v / 100) * plotH;
    const grid = el("g", { class: "grid" });
    for (const v of [0, 50, 100]) {
      grid.append(el("line", { x1: left, x2: width, y1: y(v), y2: y(v) }));
      const label = el("text", { x: 0, y: y(v) + 3 });
      label.textContent = v + "%";
      grid.append(label);
    }
    svg.append(grid);
    const base = weeks.map(() => 0);
    for (const s of series) {
      const top = s.values.map((v, i) => base[i] + v);
      const upper = top.map((v, i) => x(i) + "," + y(v)).join(" L");
      const lower = base.map((v, i) => x(i) + "," + y(v)).reverse().join(" L");
      svg.append(el("path", { d: "M" + upper + " L" + lower + " Z", fill: s.color, stroke: "var(--background)", "stroke-width": 2, "stroke-linejoin": "round" }));
      top.forEach((v, i) => { base[i] = v; });
    }
    weeks.forEach((week, i) => {
      if (i % 3 !== 0 && i !== weeks.length - 1) return;
      const label = el("text", { x: x(i), y: height - 2, "text-anchor": i === 0 ? "start" : i === weeks.length - 1 ? "end" : "middle" });
      label.textContent = week;
      svg.append(label);
    });
    const cross = el("line", { y1: 0, y2: plotH, stroke: "var(--foreground)", "stroke-width": 1, opacity: 0 });
    svg.append(cross);
    svg.onmousemove = (event) => {
      const rect = svg.getBoundingClientRect();
      const i = Math.max(0, Math.min(weeks.length - 1, Math.round(((event.clientX - rect.left - left) / plotW) * (weeks.length - 1))));
      cross.setAttribute("x1", x(i));
      cross.setAttribute("x2", x(i));
      cross.setAttribute("opacity", 0.4);
      showTip(event, "<strong>Week of " + weeks[i] + "</strong>" + series
        .slice().reverse()
        .map((s) => '<div><i style="background:' + s.color + '"></i>' + s.name + " " + s.values[i] + "%</div>")
        .join(""));
    };
    svg.onmouseleave = () => { cross.setAttribute("opacity", 0); hideTip(); };
  }
  drawArea();
  new ResizeObserver(drawArea).observe(svg);

  const days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  const heat = document.getElementById("heat");
  const steps = [8, 22, 38, 56, 76, 100];
  const shade = (step) => "color-mix(in oklab, var(--chart-1) " + steps[step] + "%, var(--muted))";
  let cells = '<span></span>';
  for (let h = 0; h < 24; h++) cells += '<span class="lbl">' + (h % 6 === 0 ? (h === 0 ? "12a" : h === 12 ? "12p" : h < 12 ? h + "a" : h - 12 + "p") : "") + "</span>";
  days.forEach((day, d) => {
    cells += '<span class="lbl">' + day + "</span>";
    for (let h = 0; h < 24; h++) {
      const work = Math.exp(-Math.pow((h - 14.5) / 4.2, 2));
      const weekend = d >= 5 ? 0.45 : d === 2 ? 1.12 : 1;
      const value = Math.round((work * weekend + 0.04) * 320);
      const step = Math.min(steps.length - 1, Math.floor((value / 360) * steps.length));
      cells += '<span class="cell" style="background:' + shade(step) + '" data-tip="' + day + " " + h + ":00 · " + value + ' turns"></span>';
    }
  });
  heat.innerHTML = cells;
  heat.onmousemove = (event) => {
    const label = event.target.dataset && event.target.dataset.tip;
    if (label) showTip(event, label); else hideTip();
  };
  heat.onmouseleave = hideTip;
  document.getElementById("scale").innerHTML = steps.map((_, i) => '<i style="background:' + shade(i) + '"></i>').join("");
</script>
</body>
</html>`;
