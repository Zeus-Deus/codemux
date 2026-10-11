import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { ComposerPendingInputPanel } from './ComposerPendingInputPanel';
import { RemoteTaskDetail } from './RemoteTaskDetail';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { useDelegationTasks } from '@/stores/delegation-store';
import { remoteFixture, taskFixture } from '@/lib/delegation.test-fixtures';
import type { PermissionRequestItem } from '@/lib/agent-chat/types';
const native=vi.hoisted(()=>({invoke:vi.fn(),listen:vi.fn().mockResolvedValue(()=>{})}));
vi.mock('@tauri-apps/api/core',()=>({invoke:native.invoke}));
vi.mock('@tauri-apps/api/event',()=>({listen:native.listen}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
const item:PermissionRequestItem={kind:'permission_request',id:'parent',seq:0,request_id:'parent',turn_id:'turn',tool_use_id:'tool',request_kind:'user-input',resolution:{state:'pending'},payload:{questions:[{question:'Parent choice?',options:[{label:'Parent safe'},{label:'Parent changed'}]}]}};
it.each(['send','deny'] as const)('FEUI-1 suspends the actual ready parent questionnaire behind a real remote dialog during %s without consuming modal controls',async action=>{
 const parentSubmit=vi.fn();const back=vi.fn();
 const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[{request_id:'remote',request_kind:'user-input',payload:{questions:[{question:'Remote answer?',options:[]}]}}]})});
 native.invoke.mockImplementation(cmd=>Promise.resolve(cmd==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:[task]));
 renderHook(()=>useDelegationTasks('pane','thread-1'));
 const parent=render(<ComposerPendingInputPanel item={item} onSubmit={parentSubmit}/>);
 fireEvent.click(parent.getByTestId('aq-option-0-0'));
 expect(parent.getByRole('radio',{name:/Parent safe/})).toBeChecked();
 render(<Dialog open><DialogContent aria-describedby={undefined}><DialogTitle>Remote task</DialogTitle><RemoteTaskDetail task={task} paneId="pane" writable onBack={back}/></DialogContent></Dialog>);
 const dialog=screen.getByRole('dialog');const remote=within(dialog);
 await waitFor(()=>expect(remote.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 for(const name of ['Back to parent conversation','Refresh task','Deny request remote']) {
  const button=remote.getByRole('button',{name});button.focus();
  for(const key of ['Enter','2','ArrowLeft','ArrowRight']) expect(fireEvent.keyDown(button,{key})).toBe(true);
  fireEvent.keyDown(button,{key:'Tab'});
 }
 fireEvent.keyDown(document,{key:'2'});
 expect(parentSubmit).not.toHaveBeenCalled();
 expect(within(parent.container).getByRole('radio',{name:/Parent safe/,hidden:true})).toBeChecked();
 fireEvent.change(remote.getByPlaceholderText('Your answer…'),{target:{value:'Exact remote answer'}});
 if(action==='send') fireEvent.submit(remote.getByRole('button',{name:'Send'}).closest('form')!);
 else fireEvent.click(remote.getByRole('button',{name:'Deny request remote'}));
 await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1));
 expect(parentSubmit).not.toHaveBeenCalled();
 fireEvent.click(remote.getByRole('button',{name:'Back to parent conversation'}));expect(back).toHaveBeenCalledTimes(1);
});
it('FEUI-1 preserves parent document shortcuts with no foreign owner and ignores handled or foreign-form events',()=>{
 const submit=vi.fn();const view=render(<><ComposerPendingInputPanel item={item} onSubmit={submit}/><form><button type="button">Foreign form</button></form></>);
 fireEvent.keyDown(document,{key:'1'});
 const handled=new KeyboardEvent('keydown',{key:'2',bubbles:true,cancelable:true});handled.preventDefault();document.dispatchEvent(handled);
 fireEvent.keyDown(view.getByRole('button',{name:'Foreign form'}),{key:'2'});
 expect(view.getByRole('radio',{name:/Parent safe/})).toBeChecked();
 fireEvent.keyDown(document,{key:'Enter'});expect(submit).toHaveBeenCalledTimes(1);
});
