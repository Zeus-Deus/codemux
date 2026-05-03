import { useCallback, useEffect, useMemo, useState } from "react";

import { AlertTriangle, Check, HelpCircle, Loader2, Trash2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { listToolPermissions, removeToolPermission } from "@/tauri/commands";
import type { PermissionRule } from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

interface Props {
  /** Active workspace's project root, or null when no project is
   *  selected (e.g. the user opened settings from the home pane).
   *  Project-scoped rules and the local-settings file are unavailable
   *  when this is null. */
  projectRoot: string | null;
}

interface Group {
  scope: PermissionRule["scope"];
  heading: string;
  filename: string;
  rules: PermissionRule[];
}

/**
 * "Permissions" settings section. Reads `~/.claude/settings.json`,
 * `<project>/.claude/settings.json`, and
 * `<project>/.claude/settings.local.json` and lists each tool-permission
 * rule grouped by scope. Removing a rule rewrites the source file in
 * place — the change applies to *new* sessions; live sessions retain
 * their cached permission map until they restart.
 */
export function PermissionsSection({ projectRoot }: Props) {
  const [rules, setRules] = useState<PermissionRule[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemoval, setPendingRemoval] = useState<PermissionRule | null>(
    null,
  );
  const [removing, setRemoving] = useState(false);

  const refresh = useCallback(() => {
    setError(null);
    listToolPermissions(projectRoot)
      .then((list) => setRules(list))
      .catch((err) => {
        setRules([]);
        setError(String(err));
      });
  }, [projectRoot]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Workspace switch resets any in-progress removal: the rule's
  // `scope` (e.g. "local") was resolved against the old project root,
  // so confirming the dialog after a switch would target the wrong
  // file. `refresh` already re-runs via the `[refresh]` dep; this
  // effect just clears the dialog state.
  useEffect(() => {
    setPendingRemoval(null);
  }, [projectRoot]);

  const groups = useMemo<Group[]>(() => groupRules(rules ?? [], projectRoot), [
    rules,
    projectRoot,
  ]);

  const handleConfirmRemove = async () => {
    if (!pendingRemoval) return;
    setRemoving(true);
    try {
      await removeToolPermission(pendingRemoval, projectRoot);
      // Optimistic local update; refetch from disk to stay in sync
      // with anything Claude CLI may have written concurrently.
      const removed = pendingRemoval;
      setRules((cur) => (cur ? cur.filter((r) => !sameRule(r, removed)) : cur));
      toast.success("Rule removed", {
        description: "New sessions will use the updated rules.",
      });
      setPendingRemoval(null);
      // Background refresh keeps us in sync with the file on disk.
      refresh();
    } catch (err) {
      toast.error("Failed to remove rule", { description: String(err) });
      // Close the dialog on failure too — the toast has the error
      // detail, and leaving it open with no inline error left users
      // confused (they tend to retry, hitting "rule not found" the
      // second time when the first attempt actually succeeded).
      setPendingRemoval(null);
    } finally {
      setRemoving(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Permissions"
        description="Tool-permission rules persist approvals so the agent doesn't keep asking for the same tools."
      />

      <p className="mb-4 flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-muted-foreground">
        <AlertTriangle
          className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-500"
          aria-hidden
        />
        <span>
          Rules in <code className="font-mono">~/.claude/settings.json</code>{" "}
          are also used by the Claude CLI when running outside Codemux.
        </span>
      </p>

      {error && (
        <p className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          Failed to load rules: {error}
        </p>
      )}

      {rules === null ? (
        <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          Loading rules…
        </div>
      ) : (
        <div className="space-y-6">
          {groups.map((group) => (
            <RuleGroup
              key={group.scope}
              group={group}
              onRemove={(rule) => setPendingRemoval(rule)}
            />
          ))}
        </div>
      )}

      <AlertDialog
        open={pendingRemoval !== null}
        onOpenChange={(open) => {
          if (!open && !removing) setPendingRemoval(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove this permission rule?</AlertDialogTitle>
            <AlertDialogDescription>
              {pendingRemoval && (
                <>
                  Removing <span className="font-mono">{describeRule(pendingRemoval)}</span> from{" "}
                  <span className="font-mono">{shortPath(pendingRemoval.source_path)}</span>.
                  New sessions will use the updated rules; sessions that are
                  already running keep their current permissions until they
                  restart.
                </>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removing}>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={handleConfirmRemove} disabled={removing}>
              {removing ? "Removing…" : "Remove rule"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function RuleGroup({
  group,
  onRemove,
}: {
  group: Group;
  onRemove: (rule: PermissionRule) => void;
}) {
  return (
    <section>
      <header className="mb-2">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {group.heading}
        </h3>
        <p className="text-[11px] text-muted-foreground/70 font-mono mt-0.5">
          {group.filename}
        </p>
      </header>
      {group.rules.length === 0 ? (
        <p className="text-xs text-muted-foreground/70 italic">
          No rules in this scope.
        </p>
      ) : (
        <ul className="space-y-1 rounded-md border border-border/50">
          {group.rules.map((rule, idx) => (
            <li
              key={`${rule.scope}-${rule.behavior}-${rule.tool_name}-${rule.rule_content ?? ""}-${idx}`}
              className={cn(
                "group flex items-center gap-3 px-3 py-2 transition-colors",
                idx > 0 && "border-t border-border/40",
                "hover:bg-accent/30",
              )}
            >
              <BehaviorIcon behavior={rule.behavior} />
              <div className="min-w-0 flex-1">
                <span className="font-mono text-sm text-foreground">
                  {rule.tool_name}
                </span>
                {rule.rule_content && (
                  <span className="ml-2 font-mono text-xs text-muted-foreground">
                    ({rule.rule_content})
                  </span>
                )}
              </div>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Remove rule"
                className="opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 transition-opacity hover:bg-destructive/10 hover:text-destructive"
                onClick={() => onRemove(rule)}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function BehaviorIcon({ behavior }: { behavior: PermissionRule["behavior"] }) {
  switch (behavior) {
    case "allow":
      return (
        <Check
          className="h-3.5 w-3.5 shrink-0 text-emerald-500"
          aria-label="Allow"
        />
      );
    case "deny":
      return (
        <X
          className="h-3.5 w-3.5 shrink-0 text-destructive"
          aria-label="Deny"
        />
      );
    case "ask":
      return (
        <HelpCircle
          className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
          aria-label="Ask"
        />
      );
  }
}

function SectionHeader({ title, description }: { title: string; description: string }) {
  return (
    <div className="mb-6">
      <h2 className="text-base font-semibold tracking-tight">{title}</h2>
      <p className="text-sm text-muted-foreground mt-1">{description}</p>
    </div>
  );
}

function groupRules(rules: PermissionRule[], projectRoot: string | null): Group[] {
  const groups: Group[] = [
    {
      scope: "user",
      heading: "User-wide",
      filename: "~/.claude/settings.json",
      rules: [],
    },
  ];
  if (projectRoot) {
    groups.push(
      {
        scope: "local",
        heading: "This project (gitignored)",
        filename: ".claude/settings.local.json",
        rules: [],
      },
      {
        scope: "project",
        heading: "This project (shared)",
        filename: ".claude/settings.json",
        rules: [],
      },
    );
  }
  for (const rule of rules) {
    const target = groups.find((g) => g.scope === rule.scope);
    if (target) target.rules.push(rule);
  }
  return groups;
}

function sameRule(a: PermissionRule, b: PermissionRule): boolean {
  return (
    a.scope === b.scope &&
    a.behavior === b.behavior &&
    a.tool_name === b.tool_name &&
    (a.rule_content ?? null) === (b.rule_content ?? null) &&
    a.source_path === b.source_path
  );
}

function describeRule(rule: PermissionRule): string {
  if (rule.rule_content) {
    return `${rule.behavior} ${rule.tool_name}(${rule.rule_content})`;
  }
  return `${rule.behavior} ${rule.tool_name}`;
}

function shortPath(path: string): string {
  // Trim the user's home prefix so the dialog stays readable.
  const home = "/home/";
  const idx = path.indexOf(home);
  if (idx === 0) {
    const rest = path.slice(home.length);
    const slash = rest.indexOf("/");
    if (slash >= 0) return `~${rest.slice(slash)}`;
  }
  return path;
}
