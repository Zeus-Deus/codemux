const path=require('path');const os=require('os');
const {app,BrowserWindow}=require('electron');
const fs=require('fs');
app.setPath('userData',path.join(os.tmpdir(),`codemux-scroll-probe-${process.pid}`));
app.whenReady().then(async()=>{
 const w=new BrowserWindow({width:1000,height:800,show:true,fullscreen:true,webPreferences:{backgroundThrottling:false}});
 let finished=false;
 w.webContents.on('console-message',(...args)=>{
 const msg=typeof args[1]==='object'?args[1].message:args[2];
 if(msg&&msg.startsWith('PROBE:')){finished=true;console.log(msg.slice(6));app.quit()}
 });
 const html=fs.readFileSync(path.join(__dirname,'page.html'),'utf8');
 await w.loadURL('data:text/html;charset=utf-8,'+encodeURIComponent(html));
 setTimeout(()=>{let count=0; const id=setInterval(()=>{
 w.webContents.sendInputEvent({type:'mouseWheel',x:500,y:400,deltaX:0,deltaY:-25.4,canScroll:true});if(++count>=250)clearInterval(id);
 },Number(process.argv[2]||8))},1600);
 setTimeout(()=>{if(!finished)app.quit()},15000);
});
