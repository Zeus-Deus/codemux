#!/usr/bin/env node
import {readFile,writeFile,mkdir,stat,copyFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gzipSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {watch} from 'node:fs';
import {build as bundle} from 'esbuild';
import {validate} from './validate.mjs';
const [command='help',directory='.']=process.argv.slice(2);
const root=resolve(directory);
async function manifest(){const bytes=await readFile(join(root,'manifest.json'));if(bytes.length>65536)throw Error('Manifest too large');return validate(JSON.parse(bytes));}
async function build(){
 await manifest();
 await bundle({stdin:{contents:"import plugin from './src/index.tsx'; import {register} from '@codemux/plugin-sdk/runtime'; register(plugin);",resolveDir:root,sourcefile:'entry.ts'},bundle:true,format:'iife',platform:'neutral',target:'es2020',jsx:'automatic',jsxImportSource:'preact',outfile:join(root,'plugin.js'),metafile:true,plugins:[{name:'no-runtime-imports',setup(b){b.onResolve({filter:/.*/},args=>{if(args.kind==='dynamic-import')throw Error('Dynamic imports are not supported')})}}]});
 await check();
}
async function check(){const m=await manifest();for(const name of ['plugin.js','README.md','LICENSE']){const info=await stat(join(root,name));if(!info.isFile()||info.size>(name==='plugin.js'?5*1024*1024:1024*1024))throw Error('Invalid '+name);}return m;}
function tarFile(name,bytes){
 const header=Buffer.alloc(512);header.write(name,0,100,'utf8');header.write('0000600\0',100);header.write('0000000\0',108);header.write('0000000\0',116);header.write(bytes.length.toString(8).padStart(11,'0')+'\0',124);header.write('00000000000\0',136);header.fill(32,148,156);header[156]=48;header.write('ustar\0',257);header.write('00',263);header.write([...header].reduce((a,b)=>a+b,0).toString(8).padStart(6,'0')+'\0 ',148);
 return Buffer.concat([header,bytes,Buffer.alloc((512-bytes.length%512)%512)]);
}
async function pack(){const m=await check();const files=[];for(const name of ['manifest.json','plugin.js','README.md','LICENSE','NOTICE','source.map']){try{files.push(tarFile(name,await readFile(join(root,name))));}catch(e){if(!['NOTICE','source.map'].includes(name)||e.code!=='ENOENT')throw e;}}
 const tar=Buffer.concat([...files,Buffer.alloc(1024)]);if(tar.length>30*1024*1024)throw Error('Expanded package too large');const bytes=gzipSync(tar,{level:9});if(bytes.length>10*1024*1024)throw Error('Compressed package too large');const out=join(root,m.id+'-'+m.version+'.cmxaddon');await writeFile(out,bytes);console.log(out+'\nsha256 '+createHash('sha256').update(bytes).digest('hex'));return out;
}
async function init(){await mkdir(root,{recursive:true});await mkdir(join(root,'src'),{recursive:true});
 const m={format:'codemux.feature-plugin',manifestVersion:1,id:'example.hello',name:'Hello',description:'An independent CodeMux plugin.',version:'1.0.0',api:'^1.0.0',entry:'plugin.js',platforms:['linux-x64','windows-x64'],author:{name:'Example',url:'https://example.com'},repository:'https://github.com/example/hello',license:'MIT',permissions:[],http:[],credentials:[],contributes:{commands:[{id:'hello',title:'Say hello',requiresWorkspace:false}],panels:[],composerActions:[],composerViews:[]},settings:[]};
 const files={'manifest.json':JSON.stringify(m,null,2),'package.json':JSON.stringify({name:m.id,version:'1.0.0',private:true,type:'module',scripts:{build:'codemux-plugin build',check:'tsc --noEmit && codemux-plugin check',pack:'codemux-plugin pack',dev:'codemux-plugin dev'},dependencies:{'@codemux/plugin-sdk':'1.0.0','preact':'10.29.8'},devDependencies:{'@codemux/plugin-cli':'1.0.0','typescript':'5.6.2'}},null,2),'src/index.tsx':"import {definePlugin} from '@codemux/plugin-sdk';\nexport default definePlugin({activate(ctx) { ctx.commands.register('hello', async () => { await ctx.ui.notify('Hello from your plugin'); }); }});\n",'tsconfig.json':JSON.stringify({compilerOptions:{target:'ES2020',module:'ESNext',moduleResolution:'Bundler',strict:true,noEmit:true,jsx:'react-jsx',jsxImportSource:'preact',skipLibCheck:true},include:['src']}),'README.md':'# Hello\n\nRun npm install, npm run build, and npm run pack. Import the .cmxaddon in Settings → Add-ons.\n','LICENSE':'MIT\nCopyright (c) '+new Date().getFullYear()+' Example\n'};
 for(const [path,contents] of Object.entries(files))await writeFile(join(root,path),contents,{flag:'wx'});
}
try{switch(command){case 'init':await init();break;case 'build':await build();break;case 'check':await check();console.log('Package valid');break;case 'pack':await pack();break;case 'dev':await build();await pack();{let running=false,again=false;const rebuild=async()=>{if(running){again=true;return}running=true;do{again=false;try{await build();await pack()}catch(e){console.error(e.message)}}while(again);running=false};watch(join(root,'src'),{recursive:true},rebuild);watch(join(root,'manifest.json'),rebuild);}break;default:console.log('codemux-plugin init <directory> | build | check | pack | dev');}}catch(e){console.error(e.message);process.exitCode=1}
