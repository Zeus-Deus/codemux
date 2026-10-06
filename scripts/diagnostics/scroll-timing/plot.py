from pathlib import Path
import json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=Path(__file__).resolve().parent
s=json.loads((ROOT/'summary.json').read_text())
fig,ax=plt.subplots(1,2,figsize=(12,4.8),gridspec_kw={'width_ratios':[1,1.65]})
fig.patch.set_facecolor('#f7f8fa')
for a in ax:a.set_facecolor('#f7f8fa');a.spines[['top','right']].set_visible(False);a.grid(axis='y',color='#dce0e6',alpha=.7);a.set_axisbelow(True)
names=['WebKitGTK','Electron shell','CodeMux mode\nclock fallback'];values=[s['full-webkit-off-8ms']['raf_median_ms'],s['equal-electron-8ms']['raf_median_ms'],s['mode-clock-off-8ms']['raf_median_ms']]
bars=ax[0].bar(names,values,color=['#bc604a','#307d9a','#587b57'],width=.65)
for b,v in zip(bars,values):ax[0].text(b.get_x()+b.get_width()/2,v+.3,f'{v:g} ms',ha='center',fontweight='bold')
ax[0].set_ylim(0,19);ax[0].set_ylabel('Median animation callback interval (ms)');ax[0].set_title('Plain page: clock cadence',loc='left',pad=15)
for name,label,color in [('full-webkit-on-2ms','Previous WebKit smoothing','#bc604a'),('mode-clock-app-wheel-2ms','CodeMux responsive glide + mode clock','#587b57'),('equal-electron-2ms','Electron native scrolling','#307d9a')]:
 d=json.loads((ROOT/'results'/(name+'.json')).read_text());end=d['wheels'][-1][0];total=sum(w[1] for w in d['wheels']);initial=d['frames'][0][1]
 points=[(t-end,(p-initial)/total*100) for t,p in d['frames'] if -550<=t-end<=350]
 ax[1].plot([p[0] for p in points],[p[1] for p in points],color=color,label=label,linewidth=2.5)
ax[1].axvline(0,color='#626d7a',linestyle='--',linewidth=1);ax[1].text(7,9,'Last wheel event',rotation=90,color='#626d7a',fontsize=9)
ax[1].axhline(100,color='#969da5',linestyle=':',linewidth=1)
ax[1].set_xlim(-550,350);ax[1].set_ylim(0,108);ax[1].set_ylabel('Requested distance covered (%)');ax[1].set_xlabel('Time relative to last delivered wheel event (ms)');ax[1].set_title('Rapid burst: before and after',loc='left',pad=15);ax[1].legend(loc='upper left',frameon=False)
fig.suptitle('Linux scroll timing on your 200 Hz display',x=.07,y=.98,ha='left',fontsize=17,fontweight='bold')
fig.text(.07,.89,'Fullscreen • 250 native injected wheel events • equal 6,350 px input • callback timing, not scanout FPS',fontsize=10,color='#566170')
fig.tight_layout(rect=[0,.04,1,.83]);fig.savefig(ROOT/'timing-chart.png',dpi=180,facecolor=fig.get_facecolor());fig.savefig(ROOT/'timing-chart.svg',facecolor=fig.get_facecolor())
