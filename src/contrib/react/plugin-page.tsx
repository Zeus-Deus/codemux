import { useUIStore } from "@/stores/ui-store";
import { UtilityPage } from "@/components/layout/utility-page";
import { PAGES_AREA } from "@/sdk/areas";
import { ContribBoundary, ContribRender } from "./boundary";
import { useContributions } from "./use-contributions";

/** A plugin page opened with host.openPage(id), in the utility-page area. */
export function PluginPage({ id }: { id: string }) {
  const page = useContributions(PAGES_AREA).find((c) => c.id === id);
  const close = () => useUIStore.getState().setShowPluginPage(null);
  return (
    <UtilityPage title={page?.title ?? "Plugin"} backLabel="Close page" onBack={close}>
      {page?.render ? (
        <ContribBoundary id={page.id} variant="block">
          <ContribRender render={page.render} />
        </ContribBoundary>
      ) : (
        <p className="p-6 text-body-sm text-muted-foreground">This plugin page is no longer available.</p>
      )}
    </UtilityPage>
  );
}
