// Matched mock-input schedule for native_glide_probe.py; no user pages.
const {app,BrowserWindow}=require('electron');
const fs=require('fs');
app.setPath('userData',`/tmp/codemux-native-glide-electron-${process.pid}`);
const output=process.argv[2];
if(!output)throw new Error('Provide an output JSON path');
app.whenReady().then(async()=>{
  const w=new BrowserWindow({width:720,height:360,show:false,focusable:false,webPreferences:{backgroundThrottling:false}});
  await w.loadURL('data:text/html,'+encodeURIComponent(`<!doctype html><style>body{margin:0}#scroll{height:100vh;overflow:auto}#content{height:10000px}</style><div id=scroll><div id=content></div></div><script>
window.probe={events:[],frames:[],total:0,prevented:0,ua:navigator.userAgent};
document.addEventListener('wheel',e=>{probe.events.push({time:performance.timeOrigin+performance.now(),deltaY:e.deltaY,trusted:e.isTrusted,cancelable:e.cancelable});probe.total+=e.deltaY;probe.prevented+=Number(e.defaultPrevented)},{passive:true,capture:true});
requestAnimationFrame(function sample(t){probe.frames.push({time:performance.timeOrigin+t,offset:document.getElementById('scroll').scrollTop});requestAnimationFrame(sample)});
</script>`));
  w.showInactive();
  const schedule=[[200,15]];
  for(let i=0;i<6;i++)schedule.push([1000+i*80,15]);
  schedule.push([2200,15],[2225,15],[2250,15],[2275,15],[2310,-15],[2335,-15]);
  schedule.push([2600,120],[2700,-120]);
  for(let i=0;i<64;i++)schedule.push([3200+i*5,15]);
  const inputs=[];
  for(const [t,value] of schedule)setTimeout(()=>{
    inputs.push([Date.now(),value]);
    w.webContents.sendInputEvent({type:'mouseWheel',x:100,y:100,deltaX:0,deltaY:-value/15*15.5,canScroll:true});
  },900+t);
  setTimeout(async()=>{
    const r=JSON.parse(await w.webContents.executeJavaScript('JSON.stringify({...probe,offset:document.getElementById("scroll").scrollTop,height:window.innerHeight})'));
    const start=inputs[0][0];
    r.inputs=inputs.map(([t,value])=>[t-start,value]);
    for(const e of r.events)e.time-=start;
    for(const f of r.frames)f.time-=start;
    r.validation={no_javascript_scroll_writes:true,no_prevented_events:r.prevented===0,fractional_pixel_paint_measured:false};
    // Record even an engine delivery discrepancy; do not leave a probe window
    // open when comparison input is coalesced or normalized differently.
    r.validation.distance_preserved=Math.abs(r.total-1131.5)<=0.01&&Math.abs(r.offset-1131.5)<=1;
    fs.writeFileSync(output,JSON.stringify(r,null,2)+'\n');
    console.log(JSON.stringify({inputs:inputs.length,events:r.events.length,total:r.total,offset:r.offset}));
    app.quit();
  },5100);
  setTimeout(()=>app.quit(),12000);
});
