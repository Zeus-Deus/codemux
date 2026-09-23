import {readFile} from 'node:fs/promises';
import {isUtf8} from 'node:buffer';
import Ajv from 'ajv';
// Mirrors the desktop's authoritative validator (addon-protocol manifest.rs),
// so `check` and `pack` accept exactly the manifests the app imports.
const schema=JSON.parse(await readFile(new URL('../schema/manifest.json',import.meta.url),'utf8'));
// A JS number cannot hold every int64, so validate() checks integer ranges.
const checkSchema=new Ajv({strict:false,allErrors:true,formats:{uint32:true,int64:true}}).compile(schema);
const i64=[-(2n**63n),2n**63n-1n];
// The exact source integers of objects from parse(); a JS number rounds past 2^53.
const sourceIntegers=new WeakMap();
const integer=(object,key)=>sourceIntegers.get(object)?.get(key)??(Number.isInteger(object[key])?BigInt(object[key]):NaN);
// The desktop reads manifest bytes with serde_json, which rejects what JSON.parse
// accepts: invalid UTF-8, duplicate keys and lone surrogates. Every manifest number
// is an integer field, and serde_json reads a fraction, an exponent, -0 or a value
// outside 64 bits as a float, which no such field accepts.
export function parse(bytes) {
 if(!isUtf8(bytes))throw Error('manifest.json is not valid UTF-8');
 // Buffer decoding needs no ICU and keeps a byte order mark, which JSON.parse rejects.
 const source=Buffer.from(bytes).toString('utf8');
 JSON.parse(source);
 // The grammar is valid, so each token is a string, a literal, a number or a bracket.
 const tokens=source.match(/"(?:[^"\\]|\\.)*"|[{}[\]]|[^\s"{}[\],:]+/g);
 let at=0;
 const string=token=>{const value=JSON.parse(token);if(!value.isWellFormed())throw Error(`Invalid string ${token} in manifest.json: lone surrogate escapes are not valid Unicode`);return value};
 const read=()=>{
  const token=tokens[at++];
  if(token==='['){const list=[];while(tokens[at]!==']')list.push(read());at++;return list}
  if(token==='{'){
   const object={},integers=new Map();
   while(tokens[at]!=='}'){
    const key=string(tokens[at++]);
    if(Object.hasOwn(object,key))throw Error(`Duplicate key "${key}" in manifest.json`);
    const value=read();
    if(typeof value==='number')integers.set(key,BigInt(tokens[at-1]));
    Object.defineProperty(object,key,{value,writable:true,enumerable:true,configurable:true});
   }
   at++;sourceIntegers.set(object,integers);return object;
  }
  if(token[0]==='"')return string(token);
  if(['true','false','null'].includes(token))return JSON.parse(token);
  if(!/^-?(0|[1-9]\d*)$/.test(token)||token==='-0'||BigInt(token)<i64[0]||BigInt(token)>i64[1])throw Error(`Invalid number ${token} in manifest.json: use a whole number within the signed 64-bit range, without a fraction or exponent`);
  return Number(token);
 };
 return read();
}
const id=/^[a-z][a-z0-9-]{0,39}$/;
// Windows reserves device names even when an extension follows.
const reserved=/^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/;
export const pluginId=value=>typeof value==='string'&&/^[a-z][a-z0-9-]{1,39}\.[a-z][a-z0-9-]{1,39}$/.test(value)&&value.split('.').every(part=>!reserved.test(part));
const unique=values=>new Set(values).size===values.length;
// Rust rejects C0 and C1 controls, and JSON with lone surrogates never parses there.
const text=(s,n)=>typeof s==='string'&&s.length>0&&s.isWellFormed()&&[...s].length<=n&&!/[\u0000-\u001f\u007f-\u009f]/.test(s);
export const icons=['file-text','git-branch','github','list','check','info','settings','book-open','link','refresh-cw','plus','circle-alert','folder','terminal','code','search'];
const https=value=>{try {const u=new URL(value);return value.isWellFormed()&&u.protocol==='https:'&&!u.username&&!u.password;}catch{return false}};
const origin=value=>{try {const u=new URL(value);return https(value)&&u.origin===value&&!u.port&&!/^\[|^[\d.]+$/.test(u.hostname)&&!u.hostname.includes('*')&&!u.hostname.endsWith('.')&&u.pathname==='/'&&!u.search&&!u.hash;}catch{return false}};
// Rust semver 1.0 grammar: strict SemVer 2.0 versions with u64 numbers, and
// comma-separated requirements where a bare version means a caret requirement.
const number='(0|[1-9]\\d*)', identifier='(?:\\d*[A-Za-z-][0-9A-Za-z-]*|0|[1-9]\\d*)', pre=`${identifier}(?:\\.${identifier})*`, build='[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*';
const u64=digits=>BigInt(digits)<=18446744073709551615n;
export function version(value) {
 const m=typeof value==='string'&&new RegExp(`^${number}\\.${number}\\.${number}(?:-${pre})?(?:\\+${build})?$`).exec(value);
 return Boolean(m)&&m.slice(1,4).every(u64);
}
const part=`(?:\\.(?:([*xX])|${number}))?`;
const comparator=new RegExp(`^(=|>=|>|<=|<|~|\\^)? *${number}${part}${part}(?:-(${pre}))?(?:\\+(${build}))? *`);
export function requirement(value) {
 if(typeof value!=='string')return null;
 let rest=value.replace(/^ +/,'');
 if(/^[*xX]/.test(rest))return /^[*xX] *$/.test(rest) ? [] : null;
 const comparators=[];
 for(;;) {
  const m=comparator.exec(rest);
  if(!m||comparators.length===32)return null;
  const [all,op,major,minorWild,minor,patchWild,patch,prerelease,metadata]=m;
  if((minorWild&&patch!==undefined)||((prerelease!==undefined||metadata!==undefined)&&patch===undefined)||![major,minor,patch].every(n=>n===undefined||u64(n)))return null;
  const wild=Boolean(minorWild||patchWild);
  comparators.push({op:op??(wild?'*':'^'),major:Number(major),minor:minor===undefined?undefined:Number(minor),patch:patch===undefined?undefined:Number(patch),pre:prerelease??''});
  rest=rest.slice(all.length);
  if(rest==='')return comparators;
  if(rest[0]!==',')return null;
  rest=rest.slice(1).replace(/^ +/,'');
 }
}
// semver's matches() for a release version, where an empty pre-release sorts last.
export function matches(comparators,[major,minor,patch]) {
 const exact=c=>major===c.major&&(c.minor===undefined||minor===c.minor)&&(c.patch===undefined||patch===c.patch)&&c.pre==='';
 const greater=c=>major!==c.major ? major>c.major : c.minor===undefined ? false : minor!==c.minor ? minor>c.minor : c.patch===undefined ? false : patch!==c.patch ? patch>c.patch : c.pre!=='';
 const less=c=>major!==c.major ? major<c.major : c.minor!==undefined&&(minor!==c.minor ? minor<c.minor : c.patch!==undefined&&patch<c.patch);
 const tilde=c=>major===c.major&&(c.minor===undefined||minor===c.minor)&&(c.patch===undefined||patch>=c.patch);
 const caret=c=>{
  if(major!==c.major)return false;
  if(c.minor===undefined)return true;
  if(c.patch===undefined)return c.major>0 ? minor>=c.minor : minor===c.minor;
  if(c.major>0)return minor!==c.minor ? minor>c.minor : patch>=c.patch;
  if(c.minor>0)return minor===c.minor&&patch>=c.patch;
  return minor===c.minor&&patch===c.patch;
 };
 const test={'=':exact,'*':exact,'>':greater,'>=':c=>exact(c)||greater(c),'<':less,'<=':c=>exact(c)||less(c),'~':tilde,'^':caret};
 return comparators.every(c=>test[c.op](c));
}
export function validate(manifest) {
 const schemaErrors=checkSchema(manifest) ? [] : checkSchema.errors;
 const failSchema=errors=>{throw Error('Manifest schema: '+errors.map(e=>e.instancePath+' '+e.message).join('; '))};
 // The rules below assume the schema's shape; value constraints the schema also
 // encodes (enum, pattern, lengths, bounds) get their more specific message there.
 const structural=schemaErrors.filter(e=>!['enum','const','pattern','format','minLength','maxLength','minimum','maximum','minItems','maxItems','uniqueItems'].includes(e.keyword));
 if (structural.length) failSchema(structural);
 const m=manifest;
 const require=(ok,message)=>{if(!ok)throw Error(message)};
 require(m.format==='codemux.feature-plugin'&&m.manifestVersion===1&&m.entry==='plugin.js','Unsupported package format: use format "codemux.feature-plugin", manifestVersion 1 and entry "plugin.js"');
 require(pluginId(m.id),'Invalid identity: use publisher.name, each part 2-40 lowercase letters, digits or hyphens starting with a letter, and not a Windows device name');
 require(text(m.name,80)&&text(m.description,240)&&text(m.author.name,80)&&text(m.license,100),'Invalid metadata: name, author and license need 1-80, 1-80 and 1-100 characters, description 1-240, without control characters');
 require(https(m.author.url)&&https(m.repository),'Invalid metadata: author.url and repository must be HTTPS URLs without credentials');
 require(version(m.version),`Invalid package version "${m.version}": use SemVer such as 1.0.0, without a v prefix or spaces`);
 const range=requirement(m.api);
 require(range,`Invalid API range "${m.api}": use comma-separated comparators such as "^1.0.0" or ">=1.0.0, <2.0.0"`);
 require(matches(range,[1,0,0]),`API range "${m.api}" does not include the supported plugin API 1.0.0`);
 require(m.platforms.length>0&&unique(m.platforms)&&unique(m.permissions),'Duplicate platforms or permissions');
 for(const [kind,max] of Object.entries({commands:20,panels:8,composerActions:8,composerViews:4})) {
  const list=m.contributes[kind];require(list.length<=max&&unique(list.map(c=>c.id)),`Duplicate or more than ${max} ${kind}`);
  for(const c of list) {
   require(id.test(c.id)&&text(c.title,80),`Invalid ${kind} entry "${c.id}": IDs need 1-40 lowercase letters, digits or hyphens starting with a letter, titles 1-80 characters`);
   require(kind==='commands'||icons.includes(c.icon),`Unknown icon "${c.icon}" for ${kind}/${c.id}; use one of: ${icons.join(', ')}`);
  }
 }
 require(m.settings.length<=50&&unique(m.settings.map(s=>s.id)),'Duplicate or more than 50 settings');
 for(const s of m.settings) {
  require(id.test(s.id)&&text(s.label,80),`Invalid setting "${s.id}"`);
  if(s.type==='string')require((s.default??'').isWellFormed()&&Buffer.byteLength(s.default??'')<=4096,`String default of "${s.id}" exceeds 4 KiB or is not valid Unicode`);
  if(s.type==='integer'){
   const [value,min,max]=['default','min','max'].map(key=>integer(s,key));
   require(i64[0]<=min&&max<=i64[1],`Integer bounds of "${s.id}" are outside the signed 64-bit range`);
   require(min<=value&&value<=max,`Integer default of "${s.id}" is outside min/max`);
  }
  if(s.type==='enum')require(s.values.length>0&&s.values.length<=50&&unique(s.values)&&s.values.includes(s.default)&&s.values.every(v=>text(v,4096)),`Invalid enum setting "${s.id}"`);
 }
 require(m.http.length<=20&&m.credentials.length<=20&&unique(m.http.map(h=>h.origin))&&unique(m.credentials.map(c=>c.id))&&unique(m.credentials.map(c=>c.origin)),'Duplicate or excessive HTTP grants');
 for(const h of m.http)require(origin(h.origin)&&h.methods.length>0&&unique(h.methods)&&(h.credential===null||m.credentials.some(c=>c.id===h.credential&&c.origin===h.origin)),`Invalid HTTP declaration "${h.origin}": use an exact lowercase HTTPS hostname origin on port 443, no wildcard, IP address, trailing dot or path`);
 for(const c of m.credentials)require(id.test(c.id)&&text(c.label,80)&&origin(c.origin)&&m.http.some(h=>h.origin===c.origin&&h.credential===c.id),`Invalid credential "${c.id}"`);
 if (schemaErrors.length) failSchema(schemaErrors);
 return m;
}
