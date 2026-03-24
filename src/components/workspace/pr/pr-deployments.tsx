import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ExternalLink } from "lucide-react";
import type { DeploymentInfo } from "@/tauri/types";
import { CollapsibleSection } from "./collapsible-section";

const STATE_COLORS: Record<string, string> = {
  success: "bg-success/20 text-success",
  active: "bg-success/20 text-success",
  pending: "bg-warning/20 text-warning",
  in_progress: "bg-warning/20 text-warning",
  queued: "bg-muted text-muted-foreground",
  failure: "bg-danger/20 text-danger",
  error: "bg-danger/20 text-danger",
  inactive: "bg-muted text-muted-foreground",
};

interface Props {
  deployments: DeploymentInfo[];
}

export function PrDeployments({ deployments }: Props) {
  const withUrls = deployments.filter((d) => d.url);
  if (withUrls.length === 0) return null;

  return (
    <CollapsibleSection label="Deployments" count={withUrls.length}>
      <div className="px-1.5 space-y-1">
        {withUrls.map((dep) => (
          <div
            key={dep.id}
            className="flex items-center gap-1.5 py-0.5 px-1"
          >
            <Badge
              className={`text-[9px] px-1 py-0 ${STATE_COLORS[dep.state] ?? "bg-muted text-muted-foreground"}`}
            >
              {dep.state}
            </Badge>
            <span className="text-xs text-foreground truncate flex-1">
              {dep.environment}
            </span>
            {dep.url && (
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-5 px-1.5 shrink-0"
                onClick={() => window.open(dep.url!, "_blank")}
              >
                <ExternalLink className="h-2.5 w-2.5 mr-0.5" />
                Preview
              </Button>
            )}
          </div>
        ))}
      </div>
    </CollapsibleSection>
  );
}
