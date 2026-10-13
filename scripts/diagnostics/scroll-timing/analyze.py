"""Summarize native wheel/frame traces; these are callback times, not scanout FPS."""
from pathlib import Path
import json, math, statistics
ROOT=Path(__file__).resolve().parent
RESULTS=ROOT/'results'
def percentile(values,q):
 return sorted(values)[max(0,math.ceil(len(values)*q)-1)]
def summarize(data):
 frames=data['frames'];wheels=data['wheels'];start,end=wheels[0][0],wheels[-1][0]
 initial=frames[0][1];direction=1 if sum(w[1] for w in wheels)>0 else -1
 distance=lambda position:direction*(position-initial)
 total=abs(sum(w[1] for w in wheels))
 intervals=[b[0]-a[0] for a,b in zip(frames,frames[1:])]
 active=[b[0]-a[0] for a,b in zip(frames,frames[1:]) if start<=a[0]<=end+250]
 before=next(p for t,p in reversed(frames) if t<=end)
 after=next(p for t,p in frames if t>=end)
 final=frames[-1][1]
 movements=[(b[0],abs(b[1]-a[1])) for a,b in zip(frames,frames[1:]) if abs(b[1]-a[1])>.1]
 active_steps=[step for t,step in movements if start<=t<=end]
 return {
  'viewport':data.get('viewport'),'wheel_count':len(wheels),'trusted_wheel_count':sum(w[3] for w in wheels),
  'observed_input_pixels':round(total,2),'input_delivery_duration_ms':round(end-start,2),
  'raf_median_ms':round(statistics.median(intervals),2),'raf_active_p95_ms':round(percentile(active,.95),2),'raf_active_max_ms':round(max(active),2),
  'distance_at_input_end_lower_pct':round(distance(before)/total*100,1),
  'distance_at_input_end_upper_pct':round(distance(after)/total*100,1),
  'final_distance_pixels':round(distance(final),2),
  'last_moving_frame_after_input_ms':round(movements[-1][0]-end,2),
  'active_moving_frame_median_step_px':round(statistics.median(active_steps),2),
  'user_agent':data['ua'],
 }
summary={}
for p in sorted(RESULTS.glob('*.json')):
 try:
  data=json.loads(p.read_text())
  if 'frames' in data and data['wheels']:summary[p.stem]=summarize(data)
 except (ValueError,KeyError):pass
(ROOT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
