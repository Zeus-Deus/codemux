#!/usr/bin/env node
import {readFile,writeFile,mkdir,stat,rename,rm} from 'node:fs/promises';
import {resolve,join,basename,dirname} from 'node:path';
import {gzipSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {watch} from 'node:fs';
import {build as bundle} from 'esbuild';
import {pluginId,validate} from './validate.mjs';
import {bundleOptions} from './bundle.mjs';
const usage='codemux-plugin init <directory> [--id publisher.name] | build [directory] [--sourcemap] | check [directory] | pack [directory] [--out <file>] | dev [directory] [--sourcemap] [--out <file>]';
const options={},positional=[];
async function manifest(){const bytes=await readFile(join(root,'manifest.json'));if(bytes.length>65536)throw Error('Manifest too large');return validate(JSON.parse(bytes));}
// Write through a temporary sibling so a watching desktop never reads a torn package.
async function writeAtomic(path,bytes){await mkdir(dirname(path),{recursive:true});const temporary=join(dirname(path),'.'+basename(path)+'.'+process.pid+'.tmp');await writeFile(temporary,bytes);for(let attempt=0;;attempt++){try{return await rename(temporary,path)}catch(e){if(attempt>=5||!['EPERM','EBUSY','EACCES'].includes(e.code)){await rm(temporary,{force:true});throw e}await new Promise(r=>setTimeout(r,50))}}}
// The entry point is always src/index.tsx; `--sourcemap` also writes source.map.
async function build(){
 await manifest();
 const result=await bundle({...bundleOptions({resolveDir:root}),outfile:join(root,'plugin.js'),sourcemap:options.sourcemap?'external':false,write:false});
 for(const file of result.outputFiles)await writeAtomic(join(root,file.path.endsWith('.map')?'source.map':'plugin.js'),file.contents);
 if(!options.sourcemap)await rm(join(root,'source.map'),{force:true});
 await check();
}
async function check(){const m=await manifest();for(const name of ['plugin.js','README.md','LICENSE']){const info=await stat(join(root,name)).catch(e=>{throw e.code==='ENOENT'?Error(`Missing ${name}${name==='plugin.js'?'; run codemux-plugin build first':''}`):e});if(!info.isFile()||info.size>(name==='plugin.js'?5*1024*1024:1024*1024))throw Error('Invalid '+name);}return m;}
function tarFile(name,bytes){
 const header=Buffer.alloc(512);header.write(name,0,100,'utf8');header.write('0000600\0',100);header.write('0000000\0',108);header.write('0000000\0',116);header.write(bytes.length.toString(8).padStart(11,'0')+'\0',124);header.write('00000000000\0',136);header.fill(32,148,156);header[156]=48;header.write('ustar\0',257);header.write('00',263);header.write([...header].reduce((a,b)=>a+b,0).toString(8).padStart(6,'0')+'\0 ',148);
 return Buffer.concat([header,bytes,Buffer.alloc((512-bytes.length%512)%512)]);
}
async function pack(out){const m=await check();const files=[];for(const name of ['manifest.json','plugin.js','README.md','LICENSE','NOTICE','source.map']){try{files.push(tarFile(name,await readFile(join(root,name))));}catch(e){if(!['NOTICE','source.map'].includes(name)||e.code!=='ENOENT')throw e;}}
 const tar=Buffer.concat([...files,Buffer.alloc(1024)]);if(tar.length>30*1024*1024)throw Error('Expanded package too large');const bytes=gzipSync(tar,{level:9});if(bytes.length>10*1024*1024)throw Error('Compressed package too large');const target=typeof out==='function'?out(m):out??join(root,m.id+'-'+m.version+'.cmxaddon');await writeAtomic(target,bytes);console.log(target+'\nsha256 '+createHash('sha256').update(bytes).digest('hex'));return target;
}
const slug=name=>name.toLowerCase().replace(/[^a-z0-9-]+/g,'-').replace(/^[^a-z]+/,'').replace(/-+/g,'-').slice(0,40).replace(/-$/,'');
const title=id=>id.split('.')[1].split('-').filter(Boolean).map(w=>w[0].toUpperCase()+w.slice(1)).join(' ');
const mit=holder=>`MIT License

Copyright (c) ${new Date().getFullYear()} ${holder}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
`;
const starter=name=>`import {
  definePlugin,
  Stack,
  Heading,
  Text,
  Button,
  PluginError,
  useEffect,
  useState,
  type ViewProps,
  type Workspace,
} from "@codemux/plugin-sdk";
// Host calls reject with a PluginError whose code says why they could not run.
const describe = (error: unknown) =>
  error instanceof PluginError ? \`\${error.code}: \${error.message}\` : "Something went wrong";
export default definePlugin({
  activate(ctx) {
    function Main({ context }: ViewProps) {
      const [workspace, setWorkspace] = useState<Workspace | null>(null);
      const [error, setError] = useState("");
      async function load(handle = context) {
        try {
          // Declared in manifest.json as the workspace.read permission.
          setWorkspace(await ctx.workspace.current(handle));
          setError("");
        } catch (e) {
          setError(describe(e));
        }
      }
      useEffect(() => {
        void load();
      }, [context]);
      return (
        <Stack spacing="md">
          <Heading>${name}</Heading>
          <Text>{workspace ? \`Project: \${workspace.name}\` : "No local project is open."}</Text>
          {error && <Text color="danger">{error}</Text>}
          <Button onPress={(event) => load(event.context)}>Refresh</Button>
        </Stack>
      );
    }
    ctx.panels.register("main", (props) => <Main {...props} />);
    ctx.commands.register("open", async (context) => {
      try {
        await ctx.panels.open("main", context);
      } catch (e) {
        console.warn(describe(e));
      }
    });
    ctx.commands.register("hello", async () => {
      // Notifications are limited to three per minute.
      try {
        await ctx.ui.notify("Hello from your plugin");
      } catch (e) {
        console.warn(describe(e));
      }
    });
  },
});
`;
async function init(){
 // Without --id, derive a placeholder from the directory name.
 const derived='example.'+slug(basename(root));
 const id=options.id??(pluginId(derived)?derived:'example.hello');
 if(!pluginId(id))throw Error(`Invalid plugin ID "${id}": pass --id publisher.name, each part 2-40 lowercase letters, digits or hyphens starting with a letter`);
 const name=title(id);
 await mkdir(join(root,'src'),{recursive:true});
 const m=validate({format:'codemux.feature-plugin',manifestVersion:1,id,name,description:'An independent CodeMux plugin.',version:'1.0.0',api:'^1.0.0',entry:'plugin.js',platforms:['linux-x64','windows-x64'],author:{name:'Example',url:'https://example.com'},repository:'https://github.com/example/'+id.split('.')[1],license:'MIT',permissions:['workspace.read'],http:[],credentials:[],contributes:{commands:[{id:'open',title:'Open '+name,requiresWorkspace:true},{id:'hello',title:'Say hello',requiresWorkspace:false}],panels:[{id:'main',title:name,icon:'file-text'}],composerActions:[],composerViews:[]},settings:[]});
 const files={'manifest.json':JSON.stringify(m,null,2)+'\n','package.json':JSON.stringify({name:m.id,version:'1.0.0',private:true,type:'module',scripts:{build:'codemux-plugin build',check:'tsc --noEmit && codemux-plugin check',pack:'codemux-plugin pack',dev:'codemux-plugin dev'},dependencies:{'@codemux/plugin-sdk':'1.0.0','preact':'10.29.8'},devDependencies:{'@codemux/plugin-cli':'1.0.0','typescript':'5.6.2'}},null,2)+'\n','src/index.tsx':starter(name),'tsconfig.json':JSON.stringify({compilerOptions:{target:'ES2020',module:'ESNext',moduleResolution:'Bundler',strict:true,noEmit:true,jsx:'react-jsx',jsxImportSource:'preact',skipLibCheck:true},include:['src']})+'\n','.gitignore':'node_modules/\ndist/\nplugin.js\nsource.map\n*.cmxaddon\n','LICENSE':mit('Example'),
  'README.md':`# ${name}\n\nA CodeMux feature plugin. Before publishing, replace the \`example\` publisher in the \`${id}\` ID, the author, repository and LICENSE holder with your own; the ID cannot change after users install it.\n\nRun \`npm install\`, \`npm run build\`, \`npm run check\` and \`npm run pack\`, then import the \`.cmxaddon\` in Settings → Add-ons. \`npm run dev\` rebuilds on every change and writes \`dist/${id}.cmxaddon\`; select that file in Developer mode.\n\nThe code lives in \`src/index.tsx\`. The panel reads the open project through the \`workspace.read\` permission declared in \`manifest.json\`; declare every permission a host call needs. Host calls can fail with a \`PluginError\` code, so handle them where they are made. See the \`@codemux/plugin-sdk\` README for the API, limits and components.\n`};
 for(const [path,contents] of Object.entries(files))await writeFile(join(root,path),contents,{flag:'wx'});
 console.log(`Created ${id} in ${root}`);
}
async function dev(){
 const out=m=>options.out?resolve(options.out):join(root,'dist',m.id+'.cmxaddon');
 let running=false,again=false;
 // A failed build is reported and the watch continues, including the first one.
 const rebuild=async()=>{if(running){again=true;return}running=true;do{again=false;try{await build();await pack(out)}catch(e){console.error(e.message)}}while(again);running=false};
 watch(join(root,'src'),{recursive:true},rebuild);
 watch(root,(event,name)=>{if(['manifest.json','README.md','LICENSE','NOTICE'].includes(String(name)))rebuild()});
 await rebuild();
 console.log('Watching src/, manifest.json, README.md, LICENSE and NOTICE. Select the package above in Developer mode.');
}
let root;
try{
 const args=process.argv.slice(2);
 for(let i=0;i<args.length;i++){const arg=args[i];if(arg==='--sourcemap')options.sourcemap=true;else if(arg==='--id'||arg==='--out'){if(i+1>=args.length)throw Error(`${arg} needs a value`);options[arg.slice(2)]=args[++i];}else if(arg.startsWith('--'))throw Error(`Unknown option ${arg}\n${usage}`);else positional.push(arg);}
 const [command='help',directory='.']=positional;root=resolve(directory);
 switch(command){case 'init':await init();break;case 'build':await build();break;case 'check':await check();console.log('Package valid');break;case 'pack':await pack(options.out&&resolve(options.out));break;case 'dev':await dev();break;default:console.log(usage);}
}catch(e){console.error(e.message);process.exitCode=1}
