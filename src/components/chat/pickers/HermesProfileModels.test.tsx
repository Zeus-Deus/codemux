import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { HermesProfileModels } from "./HermesProfileModels";
import { useHermes, type HermesProfile } from "@/stores/hermes-store";
const profile: HermesProfile = {schema_version:1, host:"local", installation:"/bin/hermes", root:"/hermes", id:"coder", home:"/hermes/profiles/coder", identity:"1"};
const catalog = {state:"ready", session:{models:{currentModelId:"service:current", availableModels:[{modelId:"service:current",name:"Native model label",_meta:{provider:"Native service"}},{modelId:"custom:named:model",name:"Unsupported named custom"}]}}};
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  useHermes.setState({selections:{},preferred:{},fixed:{},modes:{},catalogs:{}});
  vi.mocked(invoke).mockImplementation(async command => command === "hermes_binding" ? null : command === "hermes_profiles" ? [profile] : catalog);
});
afterEach(cleanup);
it("selects an existing profile, groups its native catalog and gates the known resume route", async () => {
  const onSelect=vi.fn(), onProfileChange=vi.fn();
  render(<HermesProfileModels threadId="draft" projectPath="/project" model={null} onSelect={onSelect} onProfileChange={onProfileChange}/>);
  await screen.findByRole("option",{name:"coder"});
  fireEvent.change(screen.getByLabelText("Hermes profile"), {target:{value:JSON.stringify([profile.host,profile.installation,profile.root,profile.home,profile.identity])}});
  await screen.findByRole("heading",{name:"Native service"});
  expect((screen.getByRole("button",{name:/Unsupported named custom/}) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button",{name:/Use profile default/}));
  expect(onSelect).toHaveBeenCalledWith("profile_default");
  expect(onProfileChange).toHaveBeenCalledOnce();
  expect(useHermes.getState().preferred["/project"]).toEqual(profile);
});
it("locks restored profiles and reports replacement instead of selecting a different profile", async () => {
  vi.mocked(invoke).mockImplementation(async command => command === "hermes_binding" ? {profile} : command === "hermes_profiles" ? [{...profile,identity:"replacement"}] : catalog);
  render(<HermesProfileModels threadId="existing" model="service:missing" onSelect={vi.fn()}/>);
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Profile missing or replaced"));
  expect((screen.getByLabelText("Hermes profile") as HTMLSelectElement).disabled).toBe(true);
  expect(screen.getByRole("status").textContent).toContain("service:missing");
});
it("surfaces catalog failures without erasing the profile selection", async () => {
  useHermes.setState({selections:{draft:profile}});
  vi.mocked(invoke).mockImplementation(async command => {
    if(command === "hermes_catalog") throw new Error("setup_required: credentials revoked");
    return command === "hermes_profiles" ? [profile] : null;
  });
  render(<HermesProfileModels threadId="draft" model="service:current" onSelect={vi.fn()}/>);
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("credentials revoked"));
  expect(useHermes.getState().selections.draft).toEqual(profile);
  expect(screen.queryByRole("button",{name:/Use profile default/})).toBeNull();
});
