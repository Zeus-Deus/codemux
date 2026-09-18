export type MessageDelivery = "queue" | "steer" | "interrupt";

export const MESSAGE_DELIVERY_OPTIONS = [
  {
    value: "queue",
    label: "Queue",
    description: "Send after the current turn finishes",
  },
  {
    value: "steer",
    label: "Steer",
    description: "Guide this task without stopping its tools",
  },
  {
    value: "interrupt",
    label: "Interrupt and send",
    description: "Stop current work, then send this message",
  },
] as const;

export const STEERING_UNAVAILABLE =
  "Safe steering is unavailable for this provider. Choose Queue or Interrupt and send.";

/** Only a leading, complete command is control syntax. Paths, quoted
 * examples and commands in the body remain literal user content. */
export function parseMessageDelivery(draft: string): {
  delivery: MessageDelivery;
  text: string;
  explicit: boolean;
} {
  const match = /^\s*\/(queue|steer|interrupt)(?:\s+|$)/i.exec(draft);
  return match
    ? {
        delivery: match[1].toLowerCase() as MessageDelivery,
        text: draft.slice(match[0].length).trim(),
        explicit: true,
      }
    : { delivery: "queue", text: draft.trim(), explicit: false };
}

export function withMessageDelivery(draft: string, delivery: MessageDelivery): string {
  return `/${delivery} ${parseMessageDelivery(draft).text}`;
}
