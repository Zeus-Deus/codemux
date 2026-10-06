const path=require('path');const os=require('os');
const {app,BrowserWindow}=require('electron');const fs=require('fs');
app.setPath('userData',path.join(os.tmpdir(),`codemux-scroll-app-probe-${process.pid}`));
app.whenReady().then(async()=>{
 const w=new BrowserWindow({width:1400,height:1000,show:true,fullscreen:true,webPreferences:{backgroundThrottling:false}});
 let finished=false;w.webContents.on('console-message',(...args)=>{const msg=typeof args[1]==='object'?args[1].message:args[2];if(msg&&msg.startsWith('PROBE:')){finished=true;console.log(msg.slice(6));app.quit()}});
 await w.loadURL(process.env.SCROLL_PROBE_URL || 'http://localhost:1431/');
 setTimeout(async()=>{try{
 const rect=JSON.parse(await w.webContents.executeJavaScript(fs.readFileSync(path.join(__dirname,'app-collector.js'),'utf8')));
 setTimeout(()=>{let count=0;const id=setInterval(()=>{w.webContents.sendInputEvent({type:'mouseWheel',x:Math.round(rect.x),y:Math.round(rect.y),deltaX:0,deltaY:25.4,canScroll:true});if(++count>=250)clearInterval(id)},8)},1800);
 }catch(e){console.error(e);app.quit()}},7000);
 setTimeout(()=>{if(!finished)app.quit()},25000);
});
