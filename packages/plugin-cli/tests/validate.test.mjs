// Author-side validation must agree with the desktop's manifest.rs.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {icons, validate} from '../src/validate.mjs';
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
