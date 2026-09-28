// Author-side validation must agree with the desktop's manifest.rs.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {icons, parse, validate} from '../src/validate.mjs';
const {base, cases} = JSON.parse(await readFile(new URL('./manifest-cases.json', import.meta.url), 'utf8'));
function apply(manifest, pointer, value) {
 const parts = pointer.split('/').slice(1);
 let node = manifest;
 for (const part of parts.slice(0, -1)) node = node[part];
 node[parts.at(-1)] = value;
}
function outcome(manifest) {
 try {validate(manifest); return 'valid';}
 catch (error) {return /does not include the supported plugin API/.test(error.message) ? 'incompatible' : 'invalid';}
}
for (const {name, set, expect} of cases)
 test(`manifest case: ${name}`, () => {
  const manifest = structuredClone(base);
  for (const [pointer, value] of Object.entries(set)) apply(manifest, pointer, value);
  assert.equal(outcome(manifest), expect);
 });
test('icon allowlists match the desktop validator and the SDK', async () => {
 const list = source => JSON.parse('[' + source.replaceAll("'", '"').replace(/,\s*$/, '') + ']');
 const rust = await readFile(new URL('../../../src-tauri/addon-protocol/src/manifest.rs', import.meta.url), 'utf8');
 const sdk = await readFile(new URL('../../plugin-sdk/src/ui.ts', import.meta.url), 'utf8');
 assert.deepEqual(icons, list(/pub const ICONS: &\[&str\] = &\[([^\]]*)\]/.exec(rust)[1]));
 assert.deepEqual(icons, list(/export const ICONS=\[([^\]]*)\]/.exec(sdk)[1]));
});
test('errors name the rejected value', () => {
 const manifest = structuredClone(base);
 manifest.contributes.panels[0].icon = 'star';
 assert.throws(() => validate(manifest), /Unknown icon "star" for panels\/main; use one of: file-text/);
 manifest.contributes.panels[0].icon = 'file-text';
 manifest.version = 'v1.0.0';
 assert.throws(() => validate(manifest), /Invalid package version "v1\.0\.0"/);
});
// `check` and `pack` read manifest.json bytes, which the desktop parses with
// serde_json. The cases above are parsed values and cannot express these.
test('manifest bytes are read as strictly as the desktop reads them', () => {
 const text = JSON.stringify(base);
 const settings = value => text.replace('"settings":[]', `"settings":[${value}]`);
 const integer = fields => settings(`{"id":"count","type":"integer","label":"Count",${fields}}`);
 const note = value => settings(`{"id":"note","type":"string","label":"Note","default":${value}}`);
 const invalidUtf8 = Buffer.from(text.replace('Hello', 'Hel#lo'));
 invalidUtf8[invalidUtf8.indexOf('#')] = 0xff;
 const cases = [
  ['base', text, 'valid'],
  ['fractional manifestVersion', text.replace('"manifestVersion":1', '"manifestVersion":1.0'), 'invalid'],
  ['exponent manifestVersion', text.replace('"manifestVersion":1', '"manifestVersion":1e0'), 'invalid'],
  ['integer setting', integer('"default":-5,"min":-10,"max":10'), 'valid'],
  ['exponent bound', integer('"default":5,"min":0,"max":1e6'), 'invalid'],
  ['fractional default', integer('"default":5.0,"min":0,"max":10'), 'invalid'],
  ['negative zero', integer('"default":5,"min":-0,"max":10'), 'invalid'],
  ['i64 extremes', integer('"default":9007199254740993,"min":-9223372036854775808,"max":9223372036854775807'), 'valid'],
  ['above i64', integer('"default":5,"min":0,"max":9223372036854775808'), 'invalid'],
  ['below i64', integer('"default":5,"min":-9223372036854775809,"max":10'), 'invalid'],
  ['default above max past 2^53', integer('"default":9007199254740993,"min":0,"max":9007199254740992'), 'invalid'],
  ['surrogate pair default', note('"\\ud83d\\ude00"'), 'valid'],
  ['lone surrogate default', note('"\\ud800"'), 'invalid'],
  ['lone surrogate repository', text.replace('"https://github.com/example/hello"', '"https://github.com/example/\\udc00"'), 'invalid'],
  ['duplicate key', text.replace('"name":"Hello"', '"name":"Hello","name":"Hello"'), 'invalid'],
  ['duplicate setting tag', settings('{"id":"on","type":"boolean","type":"boolean","label":"On","default":true}'), 'invalid'],
  ['invalid UTF-8', invalidUtf8, 'invalid'],
  ['byte order mark', Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from(text)]), 'invalid'],
 ];
 for (const [name, bytes, expect] of cases) {
  let result = 'valid';
  try {validate(parse(Buffer.from(bytes)));} catch {result = 'invalid';}
  assert.equal(result, expect, name);
 }
 // Parsed values carry no syntax, but must still fit the desktop's types.
 const manifest = structuredClone(base);
 manifest.settings = [{id: 'count', type: 'integer', label: 'Count', default: 0, min: 0, max: 2 ** 63}];
 assert.throws(() => validate(manifest), /outside the signed 64-bit range/);
 manifest.settings = [{id: 'note', type: 'string', label: 'Note', default: '\ud800'}];
 assert.throws(() => validate(manifest), /not valid Unicode/);
 manifest.settings = [];
 // URL parsing percent-encodes a lone surrogate, so https() checks it first.
 for (const field of ['repository', 'author.url']) {
  const copy = structuredClone(manifest);
  apply(copy, '/' + field.replace('.', '/'), 'https://github.com/example/\udc00');
  assert.throws(() => validate(copy), /author\.url and repository must be HTTPS URLs/, field);
 }
});
// Node built without ICU rejects TextDecoder's `fatal` option.
test('manifest bytes are read without ICU', () => {
 const {TextDecoder} = globalThis;
 globalThis.TextDecoder = class extends TextDecoder {
  constructor(label, options) {
   if (options?.fatal) throw Object.assign(new TypeError('"fatal" option is not supported on Node.js compiled without ICU'), {code: 'ERR_NO_ICU'});
   super(label, options);
  }
 };
 try {
  assert.equal(validate(parse(Buffer.from(JSON.stringify(base)))).id, base.id);
  assert.throws(() => parse(Buffer.from([0x7b, 0xff, 0x7d])), /not valid UTF-8/);
 } finally {globalThis.TextDecoder = TextDecoder;}
});
