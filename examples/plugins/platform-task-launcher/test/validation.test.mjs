import assert from 'node:assert/strict';
import { test } from 'node:test';
import { choices } from '../validation.mjs';
import { sourceThread } from '../../../../packages/plugin-sdk/test/worker-harness.mjs';

const defaults = {
  modelSelection: sourceThread.modelSelection,
  runtimeMode: 'auto-accept-edits',
  interactionMode: 'default',
};

test('an explicit native permission choice replaces the inherited portable mode', () => {
  const selected = choices({nativePermissionMode:'workspace-write'}, defaults, false);
  assert.equal(selected.nativePermissionMode, 'workspace-write');
  assert.equal(Object.hasOwn(selected, 'runtimeMode'), false);
  assert.deepEqual(choices({}, defaults, false).runtimeMode, 'auto-accept-edits');
});

test('an explicit portable permission choice replaces an inherited native mode', () => {
  const { runtimeMode: _runtime, ...nativeDefaults } = defaults;
  const selected = choices({runtimeMode:'approval-required'}, {
    ...nativeDefaults, nativePermissionMode:'workspace-write',
  }, false);
  assert.equal(selected.runtimeMode, 'approval-required');
  assert.equal(Object.hasOwn(selected, 'nativePermissionMode'), false);
});

test('conflicting or unsupported permission choices fail before journaling', () => {
  assert.throws(() => choices({runtimeMode:'approval-required',nativePermissionMode:'workspace-write'}, defaults, false), /not both/);
  assert.throws(() => choices({}, {...defaults,nativePermissionMode:'workspace-write'}, false), /not both/);
  assert.throws(() => choices({runtimeMode:'auto'}, defaults, false), /Unsupported runtime mode/);
  assert.throws(() => choices({nativePermissionMode:''}, defaults, false), /native permission mode/);
});
