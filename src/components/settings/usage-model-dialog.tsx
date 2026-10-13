import { useEffect, useMemo, useState } from "react";

import { Pencil, Trash2 } from "lucide-react";

import { ProviderLogo } from "@/components/chat/provider-logo";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Eyebrow } from "@/components/ui/eyebrow";
import { toast } from "@/lib/toast";
import {
  usageSetPriceOverride,
  type FlatModelUsage,
  type PriceOverride,
  type UsageBucket,
} from "@/tauri/commands";
import { UsageAreaChart } from "./usage-area-chart";
import {
  formatMoney,
  formatPercent,
  formatTokens,
  isKnownProvider,
  seriesColor,
  seriesLabel,
} from "./usage-format";
import type { UsageMetric } from "./usage-preferences";
import { UsageShareBar, costTypeSegments, tokenTypeSegments } from "./usage-share-bar";

/** Share of a model's input served from cache, or null without input.
 *  Cache writes count as misses: that input was processed in full. */
export function cacheHitRate(model: FlatModelUsage): number | null {
  const input = model.input_tokens + model.cache_read_tokens + model.cache_write_tokens;
  return input === 0 ? null : model.cache_read_tokens / input;
}

/** Effective USD per million priced tokens, or null when none were priced. */
export function costPerMillionTokens(model: FlatModelUsage): number | null {
  const priced = model.tokens - model.unpriced_tokens;
  return !model.priced || priced <= 0 ? null : (model.cost_usd / priced) * 1_000_000;
}

const RATE_FIELDS: { key: keyof PriceOverride; label: string }[] = [
  { key: "input", label: "Input" },
  { key: "output", label: "Output" },
  { key: "cache_read", label: "Cache read" },
  { key: "cache_write", label: "Cache write" },
];

/** Where the model's cost figures come from, in a sentence. */
function priceSource(model: FlatModelUsage): string {
  if (model.price_overridden) return "your price";
  if (model.provider_reported) return "the provider's reported cost";
  if (model.rates) return "Codemux's list-price table";
  return "no known price";
}

const CHART_HEIGHT_PX = 160;

/**
 * One model's usage in the selected period, opened from the breakdown.
 * The trend and mixes are this model's own slice of the same aggregation
 * the page renders, so every figure here reconciles with the table row.
 */
export function UsageModelDialog({
  model,
  buckets,
  totalCost,
  metric,
  onClose,
  onEditPrice,
}: {
  model: FlatModelUsage;
  buckets: UsageBucket[];
  totalCost: number;
  metric: UsageMetric;
  onClose: () => void;
  onEditPrice: (model: FlatModelUsage) => void;
}) {
  const [hovered, setHovered] = useState<number | null>(null);
  // Unpriced cost is unknown, not zero, so its trend shows tokens.
  const chartMetric: UsageMetric = model.priced ? metric : "tokens";
  const hitRate = cacheHitRate(model);
  const perMillion = costPerMillionTokens(model);
  const stats = [
    { label: "Cost", value: model.priced ? formatMoney(model.cost_usd) : "Unpriced" },
    { label: "Tokens", value: formatTokens(model.tokens) },
    perMillion === null ? null : { label: "Per 1M tokens", value: formatMoney(perMillion) },
    hitRate === null ? null : { label: "Cache hit", value: formatPercent(hitRate) },
    { label: "Sessions", value: model.session_count.toLocaleString() },
  ].filter((stat) => stat !== null);

  const series = useMemo(
    () => [
      {
        key: model.model,
        label: model.model,
        ...seriesColor(model.provider),
        values: model.buckets.map((slice) =>
          chartMetric === "cost" ? slice.cost_usd : slice.tokens,
        ),
      },
    ],
    [model, chartMetric],
  );
  const points = useMemo(
    () => buckets.map((b) => ({ label: b.label, subLabel: b.sub_label })),
    [buckets],
  );
  const format = (v: number) => (chartMetric === "cost" ? formatMoney(v) : formatTokens(v));

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="gap-6 sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isKnownProvider(model.provider) && (
              <ProviderLogo provider={model.provider} className="size-4" />
            )}
            <span className="font-mono">{model.model}</span>
          </DialogTitle>
          <DialogDescription>
            {seriesLabel(model.provider)}
            {model.priced && totalCost > 0
              ? ` · ${formatPercent(model.cost_usd / totalCost)} of cost`
              : ""}
            {model.reasoning_tokens > 0
              ? ` · ${formatTokens(model.reasoning_tokens)} reasoning tokens`
              : ""}
          </DialogDescription>
        </DialogHeader>

        <div className="grid grid-cols-2 gap-x-6 gap-y-4 sm:grid-cols-5">
          {stats.map((stat) => (
            <div key={stat.label} className="flex min-w-0 flex-col gap-1">
              <Eyebrow>{stat.label}</Eyebrow>
              <span className="select-text font-mono text-[1.375rem] font-semibold leading-none tabular-nums">
                {stat.value}
              </span>
            </div>
          ))}
        </div>

        <UsageAreaChart
          series={series}
          points={points}
          height={CHART_HEIGHT_PX}
          hovered={hovered}
          onHover={setHovered}
          formatValue={format}
          ariaLabel={`${model.model} ${chartMetric === "cost" ? "cost" : "tokens"} per bucket`}
        />

        <div className="grid gap-x-10 gap-y-6 sm:grid-cols-2">
          {model.priced && (
            <UsageShareBar
              label="Cost by type"
              segments={costTypeSegments(model.category_cost)}
              format={formatMoney}
            />
          )}
          <UsageShareBar
            label="Tokens by type"
            segments={tokenTypeSegments(model)}
            format={formatTokens}
          />
        </div>

        <DialogFooter className="items-center sm:justify-between">
          <span className="text-label text-muted-foreground">
            {model.unpriced_tokens > 0
              ? `${formatTokens(model.unpriced_tokens)} tokens have no known price`
              : model.rates
                ? `${RATE_FIELDS.map((f) => `${formatMoney(model.rates![f.key])} ${f.label.toLowerCase()}`).join(" · ")} per 1M · from ${priceSource(model)}`
                : `Priced from ${priceSource(model)}`}
          </span>
          <Button
            size="sm"
            variant={model.unpriced_tokens > 0 ? "default" : "outline"}
            onClick={() => onEditPrice(model)}
          >
            {model.unpriced_tokens > 0 ? "Set price" : "Edit price"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type RateDraft = Record<keyof PriceOverride, string>;

function draftFrom(rates: PriceOverride | null | undefined): RateDraft {
  const show = (n: number | undefined) => (n == null ? "" : String(Number(n.toFixed(6))));
  return {
    input: show(rates?.input),
    output: show(rates?.output),
    cache_read: show(rates?.cache_read),
    cache_write: show(rates?.cache_write),
  };
}

/** Parsed rates, or the first field that is not a non-negative number.
 *  Blank cache fields default to zero: many providers charge nothing extra. */
export function parseRateDraft(
  draft: RateDraft,
): { ok: true; price: PriceOverride } | { ok: false; field: keyof PriceOverride } {
  const price = {} as PriceOverride;
  for (const { key } of RATE_FIELDS) {
    const raw = draft[key].trim();
    const required = key === "input" || key === "output";
    if (raw === "" && !required) {
      price[key] = 0;
      continue;
    }
    const value = Number(raw);
    if (raw === "" || !Number.isFinite(value) || value < 0) return { ok: false, field: key };
    price[key] = value;
  }
  return { ok: true, price };
}

/**
 * Set, change, or remove one model's price (USD per million tokens). An
 * override wins over the built-in table and provider-reported costs, and
 * applies when usage is read, so removing it restores the original figures.
 */
export function PriceEditorDialog({
  model,
  initialRates,
  hasOverride,
  onClose,
  onSaved,
}: {
  /** `null` to let the user type the model id. */
  model: string | null;
  initialRates: PriceOverride | null;
  hasOverride: boolean;
  onClose: () => void;
  onSaved: (overrides: Record<string, PriceOverride>) => void;
}) {
  const [modelId, setModelId] = useState(model ?? "");
  const [draft, setDraft] = useState<RateDraft>(() => draftFrom(initialRates));
  const [invalid, setInvalid] = useState<keyof PriceOverride | "model" | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setInvalid(null);
  }, [draft, modelId]);

  const save = async (price: PriceOverride | null) => {
    const id = modelId.trim();
    if (!id) {
      setInvalid("model");
      return;
    }
    setSaving(true);
    try {
      onSaved(await usageSetPriceOverride(id, price));
      onClose();
    } catch (err) {
      toast.error("Could not save the price", { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  const submit = () => {
    const parsed = parseRateDraft(draft);
    if (!parsed.ok) {
      setInvalid(parsed.field);
      return;
    }
    void save(parsed.price);
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{hasOverride ? "Edit model price" : "Set model price"}</DialogTitle>
          <DialogDescription>
            USD per million tokens. Your price replaces the built-in rate for
            every past and future use of this model on this machine.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="usage-price-model" className="text-label">
              Model
            </Label>
            <Input
              id="usage-price-model"
              value={modelId}
              readOnly={model !== null}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="provider/model-id"
              aria-invalid={invalid === "model"}
              className="font-mono"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            {RATE_FIELDS.map((field) => (
              <div key={field.key} className="flex flex-col gap-1.5">
                <Label htmlFor={`usage-price-${field.key}`} className="text-label">
                  {field.label}
                </Label>
                <Input
                  id={`usage-price-${field.key}`}
                  inputMode="decimal"
                  value={draft[field.key]}
                  placeholder={field.key === "input" || field.key === "output" ? "0.00" : "0"}
                  onChange={(e) => setDraft((d) => ({ ...d, [field.key]: e.target.value }))}
                  aria-invalid={invalid === field.key}
                  className="font-mono tabular-nums"
                />
              </div>
            ))}
          </div>
          {invalid && invalid !== "model" && (
            <p className="text-label text-destructive">
              {RATE_FIELDS.find((f) => f.key === invalid)?.label} must be a number of zero or more.
            </p>
          )}
          <DialogFooter className="sm:justify-between">
            {hasOverride ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={saving}
                onClick={() => void save(null)}
              >
                Use default price
              </Button>
            ) : (
              <span />
            )}
            <div className="flex gap-2">
              <Button type="button" variant="outline" size="sm" onClick={onClose}>
                Cancel
              </Button>
              <Button type="submit" size="sm" disabled={saving}>
                Save price
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Every user price in one place, to review, edit, or remove. */
export function ModelPricesDialog({
  overrides,
  onClose,
  onEdit,
  onAdd,
  onChanged,
}: {
  overrides: Record<string, PriceOverride>;
  onClose: () => void;
  onEdit: (model: string) => void;
  onAdd: () => void;
  onChanged: (overrides: Record<string, PriceOverride>) => void;
}) {
  const entries = Object.entries(overrides).sort(([a], [b]) => a.localeCompare(b));
  const remove = async (model: string) => {
    try {
      onChanged(await usageSetPriceOverride(model, null));
    } catch (err) {
      toast.error("Could not remove the price", { description: String(err) });
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Model prices</DialogTitle>
          <DialogDescription>
            Your own rates, USD per million tokens. They price models the
            built-in table does not know, or correct one that changed.
          </DialogDescription>
        </DialogHeader>
        {entries.length === 0 ? (
          <p className="rounded-lg border border-border/60 bg-muted/30 px-4 py-6 text-center text-body-sm text-muted-foreground">
            No custom prices yet. Open an unpriced model in the breakdown, or add one here.
          </p>
        ) : (
          <div className="flex flex-col">
            <div className="grid grid-cols-[minmax(0,1fr)_repeat(4,4.5rem)_3.5rem] gap-x-2 pb-1.5 text-caption text-muted-foreground">
              <span>Model</span>
              {RATE_FIELDS.map((f) => (
                <span key={f.key} className="text-right">
                  {f.label}
                </span>
              ))}
              <span />
            </div>
            {entries.map(([model, price]) => (
              <div
                key={model}
                className="grid grid-cols-[minmax(0,1fr)_repeat(4,4.5rem)_3.5rem] items-center gap-x-2 border-t border-border/40 py-1.5"
              >
                <span className="truncate font-mono text-label">{model}</span>
                {RATE_FIELDS.map((f) => (
                  <span
                    key={f.key}
                    className="text-right font-mono text-label tabular-nums text-muted-foreground"
                  >
                    {formatMoney(price[f.key])}
                  </span>
                ))}
                <span className="flex justify-end gap-0.5">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Edit price for ${model}`}
                    onClick={() => onEdit(model)}
                  >
                    <Pencil className="size-3.5" aria-hidden />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Remove price for ${model}`}
                    onClick={() => void remove(model)}
                  >
                    <Trash2 className="size-3.5" aria-hidden />
                  </Button>
                </span>
              </div>
            ))}
          </div>
        )}
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onAdd}>
            Add price
          </Button>
          <Button size="sm" onClick={onClose}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
