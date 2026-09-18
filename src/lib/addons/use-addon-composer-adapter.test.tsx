import {afterEach,describe,expect,it,vi} from 'vitest';
import {act,cleanup,renderHook,waitFor} from '@testing-library/react';
vi.mock('./bridge',()=>({addonInvoke:vi.fn().mockResolvedValue(null)}));
import {useAddonComposerAdapter} from './use-addon-composer-adapter';
import {addonComposer} from './composer-registry';
import {useAppStore} from '@/stores/app-store';
import type {AppStateSnapshot} from '@/tauri/types';
afterEach(cleanup);
describe('controlled composer adapter',()=>{
  it('uses the latest pre-thread draft and cannot insert into a replaced surface',async()=>{
    useAppStore.setState({appState:{active_workspace_id:'workspace',workspaces:[{workspace_id:'workspace'}]} as AppStateSnapshot,pendingActiveWorkspaceId:null});
    const onDraftChange=vi.fn();
    const hook=renderHook(({draft,threadId}:{draft:string;threadId:string|null})=>useAddonComposerAdapter('workspace',threadId,draft,onDraftChange),{initialProps:{draft:'old draft',threadId:null as string|null}});
    await waitFor(()=>expect(hook.result.current.registered).toBe(true));
    const oldTarget=addonComposer(hook.result.current.id,'workspace');
    // The user types while an add-on awaits Git/network. The completion must
    // use these new controlled props, including before a chat thread exists.
    hook.rerender({draft:'new unsent text',threadId:null});
    act(()=>{oldTarget.append('brief');oldTarget.append('second');});
    expect(onDraftChange.mock.calls).toEqual([['new unsent text\nbrief'],['new unsent text\nbrief\nsecond']]);
    hook.rerender({draft:'different thread',threadId:'thread-2'});
    expect(()=>oldTarget.append('late response')).toThrow();
    hook.unmount();expect(()=>addonComposer(hook.result.current.id,'workspace')).toThrow();
    expect(onDraftChange).toHaveBeenCalledTimes(2);
  });
});
