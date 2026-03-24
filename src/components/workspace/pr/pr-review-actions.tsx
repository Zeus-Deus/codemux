import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ShieldCheck, ShieldAlert, MessageSquare } from "lucide-react";
import { submitPrReview } from "@/tauri/commands";

interface Props {
  cwd: string;
  prNumber: number;
  onSubmitted: () => void;
}

export function PrReviewActions({ cwd, prNumber, onSubmitted }: Props) {
  const [body, setBody] = useState("");
  const [submitting, setSubmitting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (event: string) => {
    setSubmitting(event);
    setError(null);
    try {
      await submitPrReview(cwd, prNumber, event, body.trim());
      setBody("");
      onSubmitted();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(null);
    }
  };

  return (
    <div className="space-y-1.5 px-1.5">
      <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground px-1">
        Review
      </span>
      <Textarea
        placeholder="Leave a review comment..."
        value={body}
        onChange={(e) => setBody(e.target.value)}
        className="text-xs resize-none h-14 min-h-14"
      />
      <div className="flex gap-1">
        <Button
          size="xs"
          variant="default"
          className="text-[10px] h-6 flex-1 bg-success/20 text-success hover:bg-success/30"
          disabled={submitting !== null}
          onClick={() => handleSubmit("approve")}
        >
          <ShieldCheck className="h-3 w-3 mr-0.5" />
          {submitting === "approve" ? "..." : "Approve"}
        </Button>
        <Button
          size="xs"
          variant="default"
          className="text-[10px] h-6 flex-1 bg-danger/20 text-danger hover:bg-danger/30"
          disabled={submitting !== null}
          onClick={() => handleSubmit("request-changes")}
        >
          <ShieldAlert className="h-3 w-3 mr-0.5" />
          {submitting === "request-changes" ? "..." : "Request"}
        </Button>
        <Button
          size="xs"
          variant="ghost"
          className="text-[10px] h-6 flex-1"
          disabled={submitting !== null || !body.trim()}
          onClick={() => handleSubmit("comment")}
        >
          <MessageSquare className="h-3 w-3 mr-0.5" />
          {submitting === "comment" ? "..." : "Comment"}
        </Button>
      </div>
      {error && (
        <p className="text-[10px] text-danger break-words px-1">{error}</p>
      )}
    </div>
  );
}
