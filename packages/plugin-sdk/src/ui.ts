import '@remote-dom/core/polyfill';
import {RemoteElement} from '@remote-dom/core/elements';
import {createRemoteComponent} from '@remote-dom/preact';
import {h, type ComponentChildren, type FunctionComponent} from 'preact';
import type {UiEvent} from './types.js';
/** Icon names accepted by the desktop; any other name stops the plugin generation. */
export const ICONS=['file-text','git-branch','github','list','check','info','settings','book-open','link','refresh-cw','plus','circle-alert','folder','terminal','code','search'] as const;
export type IconName=typeof ICONS[number];
export type Spacing='none'|'xs'|'sm'|'md'|'lg';
export type Columns=1|2|3|4|5|6|7|8|9|10|11|12;
export type HeadingLevel=1|2|3|4|5|6;
export type Size='xs'|'sm'|'md'|'lg';
export type Color='default'|'muted'|'success'|'warning'|'danger'|'accent';
export interface SelectOption {label:string; value:string}
export type UiCallback<E=UiEvent>=(event:E)=>void|Promise<void>;
/** Every property the native validator knows; each component accepts the subset it renders. */
export interface UiProps {
 children?:ComponentChildren;
 spacing?:Spacing; direction?:'horizontal'|'vertical';
 columns?:Columns; align?:'start'|'center'|'end'|'stretch'; width?:'auto'|'full'; height?:'auto'|'full';
 size?:Size; color?:Color;
 label?:string; value?:string|number|boolean; placeholder?:string; disabled?:boolean; checked?:boolean;
 title?:string; name?:IconName; level?:HeadingLevel; max?:number;
 options?:SelectOption[]; rows?:string[][]; headers?:string[]; items?:string[];
 onPress?:UiCallback; onChange?:UiCallback;
}
interface Styled {children?:ComponentChildren; size?:Size; color?:Color; width?:'auto'|'full'; height?:'auto'|'full'}
interface Layout extends Styled {spacing?:Spacing; align?:'start'|'center'|'end'|'stretch'}
export interface StackProps extends Layout {direction?:'horizontal'|'vertical'}
export interface GridProps extends Layout {columns?:Columns}
export interface CardProps extends Layout {title?:string}
export interface TextProps extends Styled {}
export interface HeadingProps extends Styled {level?:HeadingLevel}
export interface MarkdownProps extends Styled {}
export interface ButtonProps extends Styled {label?:string; disabled?:boolean; onPress?:UiCallback}
export interface TextFieldProps extends Styled {label?:string; value?:string; placeholder?:string; disabled?:boolean; onChange?:UiCallback<UiEvent&{value:string}>}
export interface SelectProps extends Styled {label?:string; value?:string; options:SelectOption[]; disabled?:boolean; onChange?:UiCallback<UiEvent&{value:string}>}
export interface CheckboxProps extends Styled {label?:string; checked?:boolean; disabled?:boolean; onChange?:UiCallback<UiEvent&{value:boolean}>}
export interface TabsProps extends Styled {label?:string; value?:string; options:SelectOption[]; disabled?:boolean; onChange?:UiCallback<UiEvent&{value:string}>}
export interface ListProps extends Styled {label?:string; items:string[]}
export interface TableProps extends Styled {label?:string; headers?:string[]; rows:string[][]}
export interface ProgressProps extends Styled {label?:string; value?:number; max?:number}
export interface IconProps {name:IconName; label?:string; color?:Color}
export interface EmptyStateProps extends Styled {title?:string}
const propertyNames=['spacing','direction','columns','align','width','height','size','color','label','value','placeholder','disabled','checked','title','name','level','max','options','rows','headers','items'];
type Dispatched=CustomEvent<UiEvent>&{respondWith?(response:unknown):void};
function component<P extends object>(name:string):FunctionComponent<P> {
 const tag='cmx-'+name.replace(/[A-Z]/g,(c,i)=>(i?'-':'')+c.toLowerCase());
 class Element extends RemoteElement<Record<string, unknown>, {}, {}, {press(detail:UiEvent):void; change(detail:UiEvent):void}> {static remoteProperties=propertyNames; static remoteEvents=['press','change'] as const;}
 customElements.define(tag,Element);
 const Remote=createRemoteComponent(tag as keyof HTMLElementTagNameMap,Element,{eventProps:{onPress:{event:'press'},onChange:{event:'change'}}});
 // Hand the callback's promise back to the SDK runtime, which reports stable
 // host rejections instead of treating them as unhandled.
 const forward=(callback:UiCallback<never>|undefined)=>callback ? (event:Dispatched)=>event.respondWith?.(callback(event.detail as never)) : undefined;
 return props=>{const {onPress,onChange}=props as {onPress?:UiCallback<never>; onChange?:UiCallback<never>}; return h(Remote,{...props,onPress:forward(onPress),onChange:forward(onChange)} as never);};
}
export const Stack=component<StackProps>('Stack'), Grid=component<GridProps>('Grid'), Card=component<CardProps>('Card'), Text=component<TextProps>('Text'), Heading=component<HeadingProps>('Heading'), Markdown=component<MarkdownProps>('Markdown'), Button=component<ButtonProps>('Button'), TextField=component<TextFieldProps>('TextField'), TextArea=component<TextFieldProps>('TextArea'), Select=component<SelectProps>('Select'), Checkbox=component<CheckboxProps>('Checkbox'), Switch=component<CheckboxProps>('Switch'), Tabs=component<TabsProps>('Tabs'), List=component<ListProps>('List'), Table=component<TableProps>('Table'), Badge=component<TextProps>('Badge'), Progress=component<ProgressProps>('Progress'), Icon=component<IconProps>('Icon'), Divider=component<{}>('Divider'), EmptyState=component<EmptyStateProps>('EmptyState');
