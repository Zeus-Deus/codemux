import {readFile} from 'node:fs/promises';
import Ajv from 'ajv';
import semver from 'semver';
const schema=JSON.parse(await readFile(new URL('../schema/manifest.json',import.meta.url),'utf8'));
const checkSchema=new Ajv({strict:false,allErrors:true,formats:{uint32:true,int64:true}}).compile(schema);
const id=/^[a-z][a-z0-9-]{0,39}$/;
const unique=values=>new Set(values).size===values.length;
const text=(s,n)=>typeof s==='string'&&s.length>0&&[...s].length<=n&&!/[\x00-\x1f\x7f]/.test(s);
const icons=['file-text','git-branch','github','list','check','info','settings','book-open','link','refresh-cw','plus','circle-alert','folder','terminal','code','search'];
const https=value=>{try {const u=new URL(value);return u.protocol==='https:'&&!u.username&&!u.password;}catch{return false}};
const origin=value=>{try {const u=new URL(value);return https(value)&&u.origin===value&&!u.port&&!/^\[|^[\d.]+$/.test(u.hostname)&&u.pathname==='/'&&!u.search&&!u.hash;}catch{return false}};
export function validate(manifest) {
 if (!checkSchema(manifest)) throw Error('Manifest schema: '+checkSchema.errors.map(e=>e.instancePath+' '+e.message).join('; '));
 const m=manifest;
 const require=(ok,message)=>{if(!ok)throw Error(message)};
 require(m.format==='codemux.feature-plugin'&&m.manifestVersion===1&&m.entry==='plugin.js','Unsupported package format');
 require(/^[a-z][a-z0-9-]{1,39}\.[a-z][a-z0-9-]{1,39}$/.test(m.id),'Invalid publisher.name identity');
 require(text(m.name,80)&&text(m.description,240)&&text(m.author.name,80)&&https(m.author.url)&&https(m.repository)&&text(m.license,100),'Invalid metadata');
 require(semver.valid(m.version)&&semver.validRange(m.api)&&semver.satisfies('1.0.0',m.api),'Unsupported version or API');
 require(m.platforms.length>0&&unique(m.platforms)&&unique(m.permissions),'Duplicate platforms or permissions');
 for(const [kind,max] of Object.entries({commands:20,panels:8,composerActions:8,composerViews:4})) {
  const list=m.contributes[kind];require(list.length<=max&&unique(list.map(c=>c.id)),'Duplicate or excessive contributions');
  for(const c of list) require(id.test(c.id)&&text(c.title,80)&&(kind==='commands'||icons.includes(c.icon)),'Invalid contribution');
 }
 require(m.settings.length<=50&&unique(m.settings.map(s=>s.id)),'Duplicate or excessive settings');
 for(const s of m.settings) {
  require(id.test(s.id)&&text(s.label,80),'Invalid setting');
  if(s.type==='string')require(Buffer.byteLength(s.default??'')<=4096,'String default too large');
  if(s.type==='integer')require(s.min<=s.default&&s.default<=s.max,'Invalid integer bounds');
  if(s.type==='enum')require(s.values.length>0&&s.values.length<=50&&unique(s.values)&&s.values.includes(s.default)&&s.values.every(v=>text(v,4096)),'Invalid enum');
 }
 require(m.http.length<=20&&m.credentials.length<=20&&unique(m.http.map(h=>h.origin))&&unique(m.credentials.map(c=>c.id))&&unique(m.credentials.map(c=>c.origin)),'Duplicate or excessive HTTP grants');
 for(const h of m.http)require(origin(h.origin)&&h.methods.length>0&&unique(h.methods)&&(h.credential===null||m.credentials.some(c=>c.id===h.credential&&c.origin===h.origin)),'Invalid HTTP declaration');
 for(const c of m.credentials)require(id.test(c.id)&&text(c.label,80)&&origin(c.origin)&&m.http.some(h=>h.origin===c.origin&&h.credential===c.id),'Invalid credential');
 return m;
}
