/** Snooze — "come back to me later" on a workspace card. The card leaves the
 *  active list and returns on its own at a wake time the user picks from a
 *  short menu.
 *
 *  This module is deliberately pure and clock-free: every entry point takes
 *  `now` as an argument. The menu, the wake sweep, and the tests then all read
 *  from one injected instant, so a test can sit on a DST boundary without
 *  faking timers and the menu can't disagree with the sweep that wakes the
 *  card. */

const HOUR_MS = 3_600_000;

/** Only ever used to *round* a difference between two local day starts (see
 *  `localDaysBetween`) — never to advance a date, which is what
 *  `atLocalTimeOnDay` exists to avoid. */
const DAY_MS = 86_400_000;

/** The hour a "this evening" snooze fires — end of a work session, not
 *  bedtime, so the workspace is still worth looking at when it returns. */
const EVENING_HOUR = 18;

/** The hour the next-morning and next-week snoozes fire: start of the next
 *  work session rather than midnight, so a woken card is waiting when the user
 *  actually sits down. */
const MORNING_HOUR = 9;

/** How close a preset may be before it stops being worth offering. A "this
 *  evening" chosen at 17:30 would fire before the user finished reading the
 *  menu, which reads as a broken snooze rather than a short one. */
const MIN_PRESET_LEAD_MS = HOUR_MS;

export interface SnoozePreset {
  /** Stable id for keys and tests. */
  id: "one-hour" | "this-evening" | "tomorrow" | "next-week";
  label: string;
  /** Absolute wake time, ms epoch. */
  at: number;
  /** The wall-clock instant `at` actually lands on, for rendering beside
   *  `label`. The relative labels say how far away a wake is and never when it
   *  is: "Next week" could be Monday morning or Sunday night as far as the user
   *  can tell, and committing to a deferral you can't read is how a workspace
   *  ends up gone for longer than anyone meant. */
  whenLabel: string;
}

/** A wall-clock time on a calendar day `dayOffset` days from `now`, built from
 *  local date *components* rather than by adding 86_400_000ms.
 *
 *  This is the whole DST story. A ms-arithmetic "+1 day" lands at 08:00 or
 *  10:00 local on the two days a year that are 23 or 25 hours long, so a
 *  "Tomorrow" snooze taken the night before a clock change wakes at the wrong
 *  hour. Handing the day number to the Date constructor lets the platform
 *  resolve the offset for the *target* day, so 09:00 local stays 09:00 local
 *  whatever happened to the clock in between. */
function atLocalTimeOnDay(now: number, dayOffset: number, hour: number): number {
  const d = new Date(now);
  // Day overflow is intentional and well-defined: `d.getDate() + 7` past the
  // end of a month rolls into the next month (and year), so no month-length
  // or leap-year special-casing is needed here.
  return new Date(
    d.getFullYear(),
    d.getMonth(),
    d.getDate() + dayOffset,
    hour,
    0,
    0,
    0,
  ).getTime();
}

/** Days from `now` to the *next* Monday. Monday itself returns 7, not 0 — a
 *  "Next week" picked on a Monday morning that woke the card the same morning
 *  would be indistinguishable from doing nothing. */
function daysToNextMonday(now: number): number {
  const dayOfWeek = new Date(now).getDay(); // 0 = Sunday
  return (8 - dayOfWeek) % 7 || 7;
}

/** Whole calendar days from `now`'s local day to `at`'s local day.
 *
 *  Both instants are collapsed to the start of their own local day before the
 *  subtraction, and the result is rounded, because two days a year are 23 and
 *  25 hours long: dividing the raw gap would put a "tomorrow" at 0.96 or 1.04
 *  days and a naive floor would then call it "today". Rounding day starts is
 *  immune to that one hour. */
function localDaysBetween(now: number, at: number): number {
  const startOfLocalDay = (ms: number) => {
    const d = new Date(ms);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  return Math.round((startOfLocalDay(at) - startOfLocalDay(now)) / DAY_MS);
}

/** "18:00" / "Tomorrow 09:00" / "Mon 09:00" — where a wake time actually lands
 *  on the user's own clock.
 *
 *  Both halves go through `Intl` rather than padded arithmetic: a 12-hour
 *  locale must read "6:00 PM" and the weekday must be the user's word for
 *  Monday, and hand-rolling either one ships one locale's conventions to
 *  everybody. The day prefix is only added once it carries information — a wake
 *  later today needs no date, and past tomorrow the weekday is what people
 *  actually plan against.
 *
 *  Pure like the rest of this module: the "which day is it" comparison is made
 *  against the caller's `now`, never `Date.now()`, so the tests can sit on a
 *  DST boundary and the menu can't disagree with the sweep. */
export function formatWakeLabel(now: number, at: number): string {
  const when = new Date(at);
  const time = when.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  const days = localDaysBetween(now, at);
  if (days <= 0) return time;
  if (days === 1) return `Tomorrow ${time}`;
  return `${when.toLocaleDateString(undefined, { weekday: "short" })} ${time}`;
}

/** The snooze menu's options for a given moment. Presets deliberately skew
 *  short: agent-work rhythms are hours (a CI run, a review, the next work
 *  session), not days.
 *
 *  Callers resolve these when the menu *opens*, not when the list renders — a
 *  preset built on the inbox's coarse tick can name a wake time half a minute
 *  in the past by the time it is clicked, and every card recomputing on every
 *  tick re-renders the whole sidebar for a menu nobody opened. */
export function computeSnoozePresets(now: number): SnoozePreset[] {
  const at = (
    id: SnoozePreset["id"],
    label: string,
    when: number,
  ): SnoozePreset => {
    const whenLabel = formatWakeLabel(now, when);
    return {
      id,
      label,
      at: when,
      // "Tomorrow · Tomorrow 09:00" says the day twice. Strip the prefix the
      // relative label already carries, and only for that preset — "In 1 hour"
      // taken at 23:30 genuinely needs its "Tomorrow", and "Next week" earns
      // its weekday, so neither can be stripped by a blanket rule.
      whenLabel:
        id === "tomorrow" ? whenLabel.replace(/^Tomorrow\s+/, "") : whenLabel,
    };
  };

  const presets: SnoozePreset[] = [
    at("one-hour", "In 1 hour", now + HOUR_MS),
  ];

  // "This evening" is the only conditional entry: it is anchored to a fixed
  // hour, so for most of the afternoon and all of the night it is either
  // imminent or already gone. Offering it then would either fire immediately
  // or, worse, resolve to a wake time in the past and never fire at all.
  const evening = atLocalTimeOnDay(now, 0, EVENING_HOUR);
  if (evening - now >= MIN_PRESET_LEAD_MS) {
    presets.push(at("this-evening", "This evening", evening));
  }

  presets.push(
    at("tomorrow", "Tomorrow", atLocalTimeOnDay(now, 1, MORNING_HOUR)),
  );
  presets.push(
    at(
      "next-week",
      "Next week",
      atLocalTimeOnDay(now, daysToNextMonday(now), MORNING_HOUR),
    ),
  );

  return presets;
}

/** "2h" / "18h" / "3d" — how long until the workspace returns. Mirrors the
 *  units and flooring of `formatElapsed` so a snoozed card and a settled card
 *  never render the same duration two different ways.
 *
 *  A snooze that is due (or overdue, because the app was closed across its
 *  wake time) reads "now" rather than a negative duration — the sweep is about
 *  to surface it, so "-3h" would be both ugly and untrue. */
export function formatTimeUntil(ms: number): string {
  if (ms <= 0) return "now";
  const totalSec = Math.floor(ms / 1000);
  // Under a minute we count seconds; rounding to minutes here would render
  // "0m" for a wake that is seconds away.
  if (totalSec < 60) return `${totalSec}s`;
  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) return `${totalMin}m`;
  const totalHours = Math.floor(totalMin / 60);
  if (totalHours < 24) {
    const mins = totalMin % 60;
    return mins > 0 ? `${totalHours}h${mins}m` : `${totalHours}h`;
  }
  // Past a day, days-plus-hours: a week-long snooze reading "168h" is a number
  // nobody can parse at a glance.
  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return hours > 0 ? `${days}d${hours}h` : `${days}d`;
}
