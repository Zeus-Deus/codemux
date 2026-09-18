import '@remote-dom/core/polyfill';
import {RemoteElement} from '@remote-dom/core/elements';
import {createRemoteComponent} from '@remote-dom/preact';
import {h, type ComponentChildren, type FunctionComponent} from 'preact';
import type {UiEvent} from './types.js';
export interface UiProps {
 children?:ComponentChildren;
 spacing?:'none'|'xs'|'sm'|'md'|'lg'; direction?:'horizontal'|'vertical';
 columns?:number; align?:'start'|'center'|'end'|'stretch'; width?:'auto'|'full'; height?:'auto'|'full';
 size?:'xs'|'sm'|'md'|'lg'; color?:'default'|'muted'|'success'|'warning'|'danger'|'accent';
 label?:string; value?:string|number|boolean; placeholder?:string; disabled?:boolean; checked?:boolean;
 title?:string; name?:string; level?:number; max?:number;
 options?:{label:string; value:string}[]; rows?:string[][]; headers?:string[]; items?:string[];
 onPress?:(event:UiEvent)=>void|Promise<void>; onChange?:(event:UiEvent)=>void|Promise<void>;
}
const propertyNames=['spacing','direction','columns','align','width','height','size','color','label','value','placeholder','disabled','checked','title','name','level','max','options','rows','headers','items'];
function component(name:string):FunctionComponent<UiProps> {
 const tag='cmx-'+name.replace(/[A-Z]/g,(c,i)=>(i?'-':'')+c.toLowerCase());
 class Element extends RemoteElement<Record<string, unknown>, {}, {}, {press(detail:UiEvent):void; change(detail:UiEvent):void}> {static remoteProperties=propertyNames; static remoteEvents=['press','change'] as const;}
 customElements.define(tag,Element);
 const Remote=createRemoteComponent(tag as keyof HTMLElementTagNameMap,Element,{eventProps:{onPress:{event:'press'},onChange:{event:'change'}}});
 return props=>h(Remote,{...props,onPress:props.onPress ? (event:CustomEvent<UiEvent>)=>props.onPress!(event.detail) : undefined,onChange:props.onChange ? (event:CustomEvent<UiEvent>)=>props.onChange!(event.detail) : undefined} as never);
}
export const Stack=component('Stack'), Grid=component('Grid'), Card=component('Card'), Text=component('Text'), Heading=component('Heading'), Markdown=component('Markdown'), Button=component('Button'), TextField=component('TextField'), TextArea=component('TextArea'), Select=component('Select'), Checkbox=component('Checkbox'), Switch=component('Switch'), Tabs=component('Tabs'), List=component('List'), Table=component('Table'), Badge=component('Badge'), Progress=component('Progress'), Icon=component('Icon'), Divider=component('Divider'), EmptyState=component('EmptyState');
