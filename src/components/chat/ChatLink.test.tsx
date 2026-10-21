import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
const { getSession, openChat, listen, callbacks } = vi.hoisted(() => ({ getSession: vi.fn(), openChat: vi.fn(), listen: vi.fn(), callbacks: [] as ((e: {payload:{thread_ids:string[]}}) => void)[] }));
vi.mock("@/tauri/commands", () => ({agentChatGetSession:getSession}));
vi.mock("./delegation-actions", () => ({openDelegatedChat:openChat}));
vi.mock("@tauri-apps/api/event", () => ({listen}));
vi.mock("@/stores/app-store", () => ({useAppStore:(selector:(s:unknown)=>unknown)=>selector({appState:{workspaces:[]}})}));
import { ChatLink } from "./ChatLink";
import { chatLinkId } from "@/lib/agent-chat/chat-links";
afterEach(cleanup);
beforeEach(() => { callbacks.length=0; vi.clearAllMocks(); getSession.mockResolvedValue({thread_id:"chat-1",title:"Current title",workspace_id:"ws"});listen.mockImplementation((_name,cb)=>{callbacks.push(cb);return Promise.resolve(vi.fn());}); });
describe("chat markdown destinations",()=>{
 it("accepts only versioned chat URLs and decodes an id",()=>{expect(chatLinkId("codemux-chat://v1/chat%2D1")).toBe("chat-1");for(const href of ["codemux-chat://v2/x","javascript:x","codemux-chat://v1/%zz","codemux-chat://v1/a/b","codemux-chat://v1/%0a"])expect(chatLinkId(href)).toBeNull();});
 it("uses the live title, opens through app navigation and never gives the webview a destination",async()=>{render(<ChatLink threadId="chat-1">old title</ChatLink>);const link=await screen.findByRole("link",{name:"Current title"});expect(link).not.toHaveAttribute("href");fireEvent.click(link);expect(openChat).toHaveBeenCalledWith("chat-1");fireEvent(link, new MouseEvent("auxclick",{button:1,bubbles:true}));expect(openChat).toHaveBeenCalledTimes(2);});
 it("refreshes on rename and renders deleted chats as unavailable",async()=>{render(<ChatLink threadId="chat-1">fallback</ChatLink>);await screen.findByText("Current title");getSession.mockResolvedValue(null);callbacks[0]({payload:{thread_ids:["chat-1"]}});await waitFor(()=>expect(screen.getByRole("link")).toHaveAttribute("title","Chat no longer available"));fireEvent.click(screen.getByRole("link"));expect(openChat).not.toHaveBeenCalled();expect(screen.getByText("fallback")).toBeInTheDocument();});
});
