import { readFileSync, writeFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../', import.meta.url)).replace(/\/$/, '');
const baselinePath = process.argv[2];
if (!baselinePath) throw new Error('Pass the baseline structural-share.ts path as the first argument');
const require = createRequire(root + '/package.json');
const ts = require('typescript');
async function load(path) {
  const source = ts.transpileModule(readFileSync(path, 'utf8'), {compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText;
  return (await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'))).shareStructural;
}
const baseline = await load(baselinePath);
const optimized = await load(root + '/src/lib/structural-share.ts');
const prev = { active_workspace_id: 'workspace-0', workspaces: Array.from({length:270},(_,i)=>({
  workspace_id:'workspace-'+i, project_root:'/projects/project-'+(i%9), title:'Workspace '+i,
  surfaces:Array.from({length:3},(_,j)=>({surface_id:`surface-${i}-${j}`,title:'Agent session',root:{kind:'split',children:Array.from({length:2},(_,k)=>({kind:'terminal',pane_id:`pane-${i}-${j}-${k}`,session_id:`session-${i}-${j}-${k}`,title:'Shell'}))}})),
  tabs:Array.from({length:3},(_,j)=>({tab_id:`tab-${i}-${j}`,surface_id:`surface-${i}-${j}`,kind:'terminal',title:'Terminal',icon:null})),
  git_info:{branch:'feature/example-'+i,ahead:0,behind:0,additions:0,deletions:0}, pane_statuses:{}, notifications_muted:false
}))};
const equal = JSON.parse(JSON.stringify(prev));
const changed = JSON.parse(JSON.stringify(prev));
changed.workspaces[269].surfaces[2].root.children[1].title = 'Changed title';
const iterations=300;
const results={workspaces:270,iterations_per_sample:iterations,samples_per_case:7,units:'milliseconds per reconciliation',bytes:Buffer.byteLength(JSON.stringify(prev)),cases:{}};
for(const [name,next] of [['unchanged',equal],['one_leaf_changed',changed]]) {
  const samples={baseline:[],optimized:[]};
  for(let warm=0;warm<100;warm++){baseline(prev,next);optimized(prev,next);}
  for(let sample=0;sample<7;sample++)for(const [label,fn] of (sample%2?[['optimized',optimized],['baseline',baseline]]:[['baseline',baseline],['optimized',optimized]])) {
    global.gc?.();
    const started=performance.now();
    for(let i=0;i<iterations;i++)fn(prev,next);
    samples[label].push((performance.now()-started)/iterations);
  }
  const median=values=>[...values].sort((a,b)=>a-b)[3];
  const b=median(samples.baseline),o=median(samples.optimized);
  results.cases[name]={samples,median_baseline:b,median_optimized:o,speedup:b/o};
}
if (process.argv[3]) writeFileSync(process.argv[3],JSON.stringify(results,null,2)+'\n');
console.log(JSON.stringify(results,null,2));
