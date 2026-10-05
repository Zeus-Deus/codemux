import { beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { hermesProfileKey, hermesModelUnavailable, pickHermesProfile, useHermes, HERMES_DEFAULT_PROFILE_SETTING, type HermesProfile, type HermesCatalog } from "./hermes-store";
import { useSettingsStore } from "./settings-store";
const profile = (id: string): HermesProfile => ({ schema_version:1, host:"local", installation:"/bin/hermes", root:"/profiles", id, home:`/profiles/${id}`, identity:id });
const catalog = (model: string): HermesCatalog => ({state:"ready",message:null,session:{models:{currentModelId:model,availableModels:[]}}});
beforeEach(() => {vi.mocked(invoke).mockReset();useHermes.setState({selections:{},preferred:{},fixed:{},catalogs:{}});useSettingsStore.setState({settings:{}});});
describe("Hermes profile catalogs", () => {
  it("keeps two profiles isolated and rejects an older refresh for the same profile", async () => {
    const pending: Array<(v: HermesCatalog) => void> = [];
    vi.mocked(invoke).mockImplementation(() => new Promise(resolve => pending.push(resolve as (v:HermesCatalog)=>void)));
    const a=profile("a"), b=profile("b");
    const older=useHermes.getState().refresh(a), other=useHermes.getState().refresh(b), newer=useHermes.getState().refresh(a);
    pending[2](catalog("a-new"));await newer;
    pending[1](catalog("b"));await other;
    pending[0](catalog("a-old"));await older;
    expect(useHermes.getState().catalogs[hermesProfileKey(a)].value?.session.models?.currentModelId).toBe("a-new");
    expect(useHermes.getState().catalogs[hermesProfileKey(b)].value?.session.models?.currentModelId).toBe("b");
  });
  it("restores immutable native identity and keeps future project preference separate", async () => {
    const a=profile("a"), b=profile("b");useHermes.getState().select("draft","project",a);
    vi.mocked(invoke).mockResolvedValue({profile:a});await useHermes.getState().restore("live");
    useHermes.getState().select("live","project",b);useHermes.getState().select("new","project",b);
    expect(useHermes.getState().selections.live).toEqual(a);
    expect(useHermes.getState().preferred.project).toEqual(b);
  });
  it("does not turn discovery failure into an empty successful catalog", async () => {
    vi.mocked(invoke).mockRejectedValue("setup_required: ACP dependencies missing");const p=profile("a");await useHermes.getState().refresh(p);
    expect(useHermes.getState().catalogs[hermesProfileKey(p)]).toEqual({loading:false,error:"setup_required: ACP dependencies missing",value:null});
  });
  it("blocks ambiguous custom routes without splitting opaque model IDs", () => {
    expect(hermesModelUnavailable("custom:fixture:gpt-4.1")).toBe(true);
    expect(hermesModelUnavailable("custom:gpt-4.1")).toBe(false);
    expect(hermesModelUnavailable("openrouter:org/model:variant")).toBe(false);
  });
});
describe("pickHermesProfile", () => {
  it("uses the only installed profile without a configured default", () => {
    expect(pickHermesProfile([profile("default")], "project")).toEqual(profile("default"));
    expect(pickHermesProfile([profile("a"), profile("b")], "project")).toBeNull();
  });
  it("prefers the project's last explicit pick, then the Settings default", () => {
    const a=profile("a"), b=profile("b"), c=profile("c");
    useSettingsStore.setState({settings:{[HERMES_DEFAULT_PROFILE_SETTING]:"b"}});
    expect(pickHermesProfile([a,b,c], "project")).toEqual(b);
    useHermes.getState().select("draft","project",c);
    expect(pickHermesProfile([a,b,c], "project")).toEqual(c);
    expect(pickHermesProfile([a,b,c], "other")).toEqual(b);
  });
  it("ignores a default or remembered profile that no longer exists", () => {
    useSettingsStore.setState({settings:{[HERMES_DEFAULT_PROFILE_SETTING]:"gone"}});
    useHermes.setState({preferred:{project:profile("removed")}});
    expect(pickHermesProfile([profile("a"), profile("b")], "project")).toBeNull();
    expect(pickHermesProfile([profile("a")], "project")).toEqual(profile("a"));
  });
  it("does not remember automatic picks as the project's preference", () => {
    useHermes.getState().select("draft","project",profile("a"),false);
    expect(useHermes.getState().selections.draft).toEqual(profile("a"));
    expect(useHermes.getState().preferred.project).toBeUndefined();
  });
});
