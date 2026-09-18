import type {ComponentChildren} from 'preact';
export type ContextHandle = string & {readonly __context: unique symbol};
export type Json = null | boolean | number | string | Json[] | {[key:string]:Json};
export type Disposer = () => void;
export type Handler = (context:ContextHandle) => void | Promise<void>;
export interface ViewProps {viewId:string; context:ContextHandle}
export type ViewRenderer = (props:ViewProps) => ComponentChildren;
export type StorageScope = {scope:'global'} | {scope:'workspace'; context:ContextHandle};
export interface Workspace {id:string; name:string; rootName:string; location:'local'}
export interface GitSummary {branch:string|null; ahead:number; behind:number; staged:number; unstaged:number; untracked:number; conflicts:number; paths:string[]; truncated:boolean}
export interface HttpRequest {origin:string; path:string; method:'GET'|'POST'|'PUT'|'PATCH'|'DELETE'; headers?:Record<string,string>; body?:string}
export interface HttpResponse {status:number; headers:Record<string,string>; body:string}
export type ErrorCode = 'PERMISSION_DENIED'|'CONTEXT_STALE'|'NO_WORKSPACE'|'REMOTE_UNSUPPORTED'|'NO_COMPOSER'|'INTERACTION_REQUIRED'|'NOT_A_GIT_REPO'|'INCOMPATIBLE_API'|'RESOURCE_LIMIT'|'TIMEOUT'|'PLUGIN_STOPPED'|'INVALID_MESSAGE'|'CREDENTIAL_REQUIRED'|'NETWORK_DENIED'|'STORAGE_UNAVAILABLE';
export class PluginError extends Error {constructor(public readonly code:ErrorCode, message:string) {super(message); this.name='PluginError'}}
export interface PluginContext {
 commands:{register(id:string,handler:Handler):Disposer};
 panels:{register(id:string,renderer:ViewRenderer):Disposer; open(id:string,context:ContextHandle):Promise<void>};
 composerActions:{register(id:string,handler:Handler):Disposer};
 composerViews:{register(id:string,renderer:ViewRenderer):Disposer; open(id:string,context:ContextHandle):Promise<void>};
 workspace:{current(context:ContextHandle):Promise<Workspace|null>; subscribe(callback:(context:ContextHandle|null)=>void):Disposer};
 git:{summary(context:ContextHandle):Promise<GitSummary>};
 composer:{appendText(context:ContextHandle,text:string):Promise<number>};
 settings:{get():Promise<Record<string,Json>>; subscribe(callback:(settings:Record<string,Json>)=>void):Disposer};
 storage:{get(scope:StorageScope,key:string):Promise<Json>; set(scope:StorageScope,key:string,value:Json):Promise<void>; delete(scope:StorageScope,key:string):Promise<void>};
 http:{fetch(context:ContextHandle,request:HttpRequest):Promise<HttpResponse>};
 links:{open(url:string,context:ContextHandle):Promise<void>};
 ui:{notify(message:string):Promise<void>};
}
export interface Plugin {activate(context:PluginContext):void|Promise<void>; deactivate?():void|Promise<void>}
export interface UiEvent {context:ContextHandle; value?:string|boolean}
