import {afterEach,beforeEach,describe,expect,it,vi} from 'vitest';
import {act,cleanup,render,renderHook,waitFor} from '@testing-library/react';
import {StrictMode,useLayoutEffect,type MutableRefObject} from 'react';
vi.mock('./bridge',()=>({addonInvoke:vi.fn().mockResolvedValue(null)}));
import {useAddonComposerAdapter,type AddonComposerBinding} from './use-addon-composer-adapter';
import {addonComposer,composerForWorkspace,type ComposerTarget} from './composer-registry';
import {addonInvoke} from './bridge';
import {useAppStore} from '@/stores/app-store';
import type {AppStateSnapshot} from '@/tauri/types';
beforeEach(()=>{
  vi.mocked(addonInvoke).mockReset().mockResolvedValue(null);
  useAppStore.setState({appState:{active_workspace_id:'workspace',workspaces:[{workspace_id:'workspace'}]} as AppStateSnapshot,pendingActiveWorkspaceId:null});
});
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
  it('retires the old thread\'s target in the same commit that retargets the draft',async()=>{
    // A mounted composer can be rebound from one thread to another. An
    // append that lands between that commit and the passive effects (here a
    // sibling's layout effect in the very same commit) must not reach the
    // new thread's draft.
    const binding={current:null} as MutableRefObject<AddonComposerBinding|null>;
    const oldTarget={current:null} as MutableRefObject<ComposerTarget|null>;
    const errors:unknown[]=[];
    function Composer({threadId,draft,onDraftChange}:{threadId:string;draft:string;onDraftChange:(text:string)=>void}){
      binding.current=useAddonComposerAdapter('workspace',threadId,draft,onDraftChange);
      return null;
    }
    function LateAppend({threadId}:{threadId:string}){
      useLayoutEffect(()=>{
        if(threadId!=='thread-b'||!oldTarget.current)return;
        try{oldTarget.current.append('late result');}catch(error){errors.push(error);}
      },[threadId]);
      return null;
    }
    const onA=vi.fn(),onB=vi.fn();
    const view=render(<><Composer threadId="thread-a" draft="draft a" onDraftChange={onA}/><LateAppend threadId="thread-a"/></>);
    await waitFor(()=>expect(binding.current?.registered).toBe(true));
    oldTarget.current=addonComposer(binding.current!.id,'workspace');
    act(()=>{view.rerender(<><Composer threadId="thread-b" draft="draft b" onDraftChange={onB}/><LateAppend threadId="thread-b"/></>);});
    expect(onB).not.toHaveBeenCalled();
    expect(onA).not.toHaveBeenCalled();
    expect(errors).toEqual([expect.objectContaining({data:{code:'CONTEXT_STALE'}})]);
    // The new thread gets its own registration.
    await waitFor(()=>expect(binding.current?.registered).toBe(true));
    expect(binding.current!.id).not.toBe('');
    expect(()=>addonComposer(binding.current!.id,'workspace')).not.toThrow();
  });
  it('keeps the specific registration failure as the unavailable reason',async()=>{
    vi.mocked(addonInvoke).mockImplementation((command)=>command==='addon_composer_register'
      ?Promise.reject({message:'Add-ons are unavailable in remote workspaces',data:{code:'REMOTE_UNSUPPORTED'}})
      :Promise.resolve(null));
    const hook=renderHook(()=>useAddonComposerAdapter('workspace',null,'',vi.fn()));
    await waitFor(()=>expect(hook.result.current.unavailable).toBe('Add-ons are unavailable in remote workspaces'));
    expect(hook.result.current.registered).toBe(false);
    expect(composerForWorkspace('workspace')).toBeNull();
  });
  it('explains a draft outside any workspace without asking the broker',()=>{
    const hook=renderHook(()=>useAddonComposerAdapter(null,null,'',vi.fn()));
    expect(hook.result.current).toEqual({id:'',registered:false,unavailable:'Open a local workspace to use add-on actions'});
    expect(addonInvoke).not.toHaveBeenCalled();
  });
  it('never closes the live registration when effects run twice on mount',async()=>{
    // Development double-invokes effects; a late close from the first run
    // must not unregister the second run's composer in the broker.
    const hook=renderHook(()=>useAddonComposerAdapter('workspace',null,'draft',vi.fn()),{wrapper:StrictMode});
    await waitFor(()=>expect(hook.result.current.registered).toBe(true));
    await act(async()=>{await Promise.resolve();});
    const live=hook.result.current.id;
    const closed=vi.mocked(addonInvoke).mock.calls.filter(([command,args])=>command==='addon_composer_closed'&&(args as {composerId:string}).composerId===live);
    expect(closed).toEqual([]);
    expect(composerForWorkspace('workspace')).toBe(live);
  });
});
