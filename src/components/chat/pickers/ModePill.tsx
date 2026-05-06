import {
  Bug,
  ListTodo,
  MessageCircleQuestion,
  X,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";

/** The three concrete modes that render a pill. `default` doesn't —
 *  the parent hides the pill entirely when mode is default. */
export type ActivePillMode = "plan" | "ask" | "debug";

interface Props {
  mode: ActivePillMode;
  /** Click handler for the X button. Parent handles the live
   *  permission-mode restore (Plan) or prompt-wrapper tear-down
   *  (Ask / Debug). */
  onRemove: () => void;
  /** Click handler for the pill body itself. Optional — some surfaces
   *  wire a "show mode details" popover here; the composer doesn't
   *  need it. */
  onClick?: () => void;
}

interface ModeConfig {
  label: string;
  /** Tailwind bg class at 15% opacity per the locked Research 2
   *  decision. */
  bg: string;
  /** Foreground text color — the same token the bg derives from,
   *  full opacity. Keeps pills readable on light + dark themes. */
  text: string;
  icon: LucideIcon;
}

/** Color mapping locked by Research 2: Plan = --primary,
 *  Debug = --danger, Ask = --success. All pills use the muted 15%
 *  fill per the chat-ui minimalism rule. */
export const MODE_CONFIG: Record<ActivePillMode, ModeConfig> = {
  plan: {
    label: "Plan",
    bg: "bg-primary/15",
    text: "text-primary",
    icon: ListTodo,
  },
  ask: {
    label: "Ask",
    bg: "bg-success/15",
    text: "text-success",
    icon: MessageCircleQuestion,
  },
  debug: {
    label: "Debug",
    bg: "bg-danger/15",
    text: "text-danger",
    icon: Bug,
  },
};

/**
 * Compact chip rendered inside the composer footer when a mode pill
 * is active. Replaces the `+ Mode` dropdown trigger while present.
 * Contains an icon, label, and a small X button that removes the
 * mode (restoring the pre-pill permission-mode on Plan, or just
 * flipping `mode → "default"` for Ask/Debug in later stages).
 *
 * Stage 3 only renders the Plan variant in practice; the Ask and
 * Debug configurations are defined so Stages 4 and 6 can flip their
 * pills on without a component change.
 */
export function ModePill({ mode, onRemove, onClick }: Props) {
  const config = MODE_CONFIG[mode];
  const Icon = config.icon;
  return (
    <div
      className={cn(
        // Match the padding + height of sibling picker triggers
        // (ModelPicker, ReasoningPicker, PermissionModePicker) which
        // all use `px-2.5 py-1`. The colored `bg-*/15` fill stays —
        // that's the pill's status affordance.
        "inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs",
        config.bg,
        config.text,
      )}
      role="status"
      aria-label={`${config.label} mode active`}
      onClick={onClick}
    >
      <Icon className="h-3 w-3" aria-hidden />
      <span>{config.label}</span>
      <button
        type="button"
        onClick={(e) => {
          // Stop propagation so the pill-body `onClick` doesn't fire
          // right before the X button tears the pill down.
          e.stopPropagation();
          onRemove();
        }}
        className="ml-0.5 rounded p-0.5 hover:bg-foreground/10"
        aria-label={`Remove ${config.label} mode`}
      >
        <X className="h-2.5 w-2.5" />
      </button>
    </div>
  );
}
