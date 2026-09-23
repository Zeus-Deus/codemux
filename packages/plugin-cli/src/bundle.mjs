// esbuild options shared by `build` and the repository's native SDK test, so
// both produce the same single QuickJS IIFE. The neutral platform avoids
// browser- or Node-only package variants; `module`/`main` still resolve packages
// without an exports map, and the constant NODE_ENV removes the need for the
// absent `process` global.
export function bundleOptions({entry='./src/index.tsx',runtime='@codemux/plugin-sdk/runtime',resolveDir}) {
 return {stdin:{contents:`import plugin from ${JSON.stringify(entry)}; import {register} from ${JSON.stringify(runtime)}; register(plugin);`,resolveDir,sourcefile:'entry.ts'},bundle:true,format:'iife',platform:'neutral',mainFields:['module','main'],define:{'process.env.NODE_ENV':'"production"'},target:'es2020',jsx:'automatic',jsxImportSource:'preact',plugins:[{name:'no-runtime-imports',setup(b){b.onResolve({filter:/.*/},args=>{if(args.kind==='dynamic-import')throw Error('Dynamic imports are not supported')})}}]};
}
