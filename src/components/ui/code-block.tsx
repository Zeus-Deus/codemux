import { CheckIcon, CopyIcon } from "lucide-react";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

/**
 * shadcn/ui Code Block (October 2026, radix-nova), on app tokens: a bordered
 * shell with a title row, a copy action and a mono body. For raw output and
 * JSON dumps; chat code keeps ChatCodeBlock and its highlighter.
 */
function CodeBlock({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="code-block"
      className={cn(
        "group/code-block relative flex w-full min-w-0 flex-col overflow-hidden rounded-lg border border-input text-body-sm text-foreground dark:bg-input/30",
        className,
      )}
      {...props}
    />
  );
}

function CodeBlockHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="code-block-header"
      className={cn(
        "flex min-w-0 shrink-0 items-center gap-2 px-3 pt-2.5 text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

function CodeBlockTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="code-block-title"
      className={cn(
        "min-w-0 truncate text-label font-medium text-muted-foreground [&_svg]:me-1.5 [&_svg]:inline-block [&_svg]:size-3.5 [&_svg]:align-middle",
        className,
      )}
      {...props}
    />
  );
}

function CodeBlockCopyButton({ value }: { value: string }) {
  const [copied, setCopied] = React.useState(false);
  React.useEffect(() => {
    if (!copied) return;
    const timeout = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timeout);
  }, [copied]);
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      aria-label={copied ? "Copied" : "Copy"}
      className="ms-auto -me-1.5 -my-1"
      onClick={() => {
        // The shared helper covers the plain-HTTP remote client, where
        // `navigator.clipboard` does not exist.
        void copyToClipboard(value).then((ok) => {
          if (ok) setCopied(true);
          else toast.error(COPY_FAILED_MESSAGE);
        });
      }}
    >
      {copied ? <CheckIcon /> : <CopyIcon />}
    </Button>
  );
}

function CodeBlockContent({ className, ...props }: React.ComponentProps<"pre">) {
  return (
    <pre
      data-slot="code-block-content"
      tabIndex={0}
      className={cn(
        "min-w-0 overflow-auto px-3 pt-2 pb-3 font-mono text-label leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
      {...props}
    />
  );
}

export { CodeBlock, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader, CodeBlockTitle };
