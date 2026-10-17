/**
 * Import-specifier handling for disk plugins, ported from Hermes Desktop's
 * runtime loader. A plain regex finds every `from '…'`, `import '…'` and
 * `import('…')`, but a match only counts when it sits in CODE: strings,
 * template text, comments and regex literals are excluded, so a plugin's own
 * copy (`'Copy keys from'`) is never read as an import or rewritten.
 */

const importSpecifierRe = () => /(from\s*|import\s*\(\s*|import\s+)(['"])([^'"]+)\2/g;

/** Keywords after which a `/` opens a regex literal, never a division. */
const regexKeywordRe =
  /^(?:await|case|delete|do|else|in|instanceof|new|of|return|throw|typeof|void|yield)$/;

/** True when the `/` at `slash` opens a regex literal (the previous
 *  significant char cannot end a value). Standard heuristic. */
function isRegexStart(source: string, slash: number): boolean {
  let j = slash - 1;
  while (j >= 0 && /\s/.test(source[j])) j -= 1;
  if (j < 0) return true;
  const prev = source[j];
  // Postfix `++`/`--` ends a value (division); a lone `+`/`-` cannot.
  if (prev === "+" || prev === "-") return source[j - 1] !== prev;
  if (prev === ")" || prev === "]" || prev === "'" || prev === '"' || prev === "`") return false;
  // `} /re/` at statement start is real code; dividing a block is not.
  if (prev === "}") return true;
  if (/[A-Za-z0-9_$]/.test(prev)) {
    let k = j;
    while (k >= 0 && /[A-Za-z0-9_$]/.test(source[k])) k -= 1;
    // `x.return / 2` divides a property.
    if (source[k] === ".") return false;
    return regexKeywordRe.test(source.slice(k + 1, j + 1));
  }
  return true;
}

/** End offset (exclusive) of the regex literal opened at `slash`, or -1 when
 *  it never closes on this line (so the `/` was a division). */
function regexEnd(source: string, slash: number): number {
  let j = slash + 1;
  let inClass = false;
  while (j < source.length) {
    const c = source[j];
    if (c === "\\") {
      j += 2;
    } else if (c === "\n") {
      return -1;
    } else if (c === "[") {
      inClass = true;
      j += 1;
    } else if (c === "]") {
      inClass = false;
      j += 1;
    } else if (c === "/" && !inClass) {
      j += 1;
      while (j < source.length && /[A-Za-z]/.test(source[j])) j += 1;
      return j;
    } else {
      j += 1;
    }
  }
  return -1;
}

/** Character ranges of `source` that are code. */
export function codeRanges(source: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  const stack: Array<"expr" | "template"> = [];
  let state: "block-comment" | "code" | "double" | "line-comment" | "single" | "template" = "code";
  let codeStart = 0;
  let i = 0;
  const closeCode = (end: number) => {
    if (end > codeStart) ranges.push([codeStart, end]);
  };

  while (i < source.length) {
    const ch = source[i];
    const next = i + 1 < source.length ? source[i + 1] : "";

    if (state === "code") {
      if (ch === "/" && next === "/") {
        closeCode(i);
        state = "line-comment";
        i += 2;
      } else if (ch === "/" && next === "*") {
        closeCode(i);
        state = "block-comment";
        i += 2;
      } else if (ch === "'") {
        closeCode(i);
        state = "single";
        i += 1;
      } else if (ch === '"') {
        closeCode(i);
        state = "double";
        i += 1;
      } else if (ch === "`") {
        closeCode(i);
        stack.push("template");
        state = "template";
        i += 1;
      } else if (ch === "/") {
        const end = isRegexStart(source, i) ? regexEnd(source, i) : -1;
        if (end > 0) {
          closeCode(i);
          i = end;
          codeStart = i;
        } else {
          i += 1;
        }
      } else if (ch === "}" && stack[stack.length - 1] === "expr") {
        closeCode(i);
        stack.pop();
        state = "template";
        i += 1;
      } else {
        i += 1;
      }
      continue;
    }

    if (state === "line-comment") {
      if (ch === "\n") {
        state = "code";
        codeStart = i;
      }
      i += 1;
      continue;
    }

    if (state === "block-comment") {
      if (ch === "*" && next === "/") {
        i += 2;
        state = "code";
        codeStart = i;
      } else {
        i += 1;
      }
      continue;
    }

    if (state === "single" || state === "double") {
      if (ch === "\\") {
        i += 2;
      } else if (ch === (state === "single" ? "'" : '"')) {
        i += 1;
        state = "code";
        codeStart = i;
      } else if (ch === "\n") {
        // Unterminated literal: recover as code so one stray quote can't
        // swallow the rest of the file.
        i += 1;
        state = "code";
        codeStart = i;
      } else {
        i += 1;
      }
      continue;
    }

    // Template-literal text.
    if (ch === "\\") {
      i += 2;
    } else if (ch === "$" && next === "{") {
      stack.push("expr");
      state = "code";
      i += 2;
      codeStart = i;
    } else if (ch === "`") {
      stack.pop();
      state = "code";
      i += 1;
      codeStart = i;
    } else {
      i += 1;
    }
  }

  closeCode(source.length);
  return ranges;
}

function inCode(ranges: Array<[number, number]>, at: number): boolean {
  for (const [start, end] of ranges) {
    if (at < start) return false;
    if (at < end) return true;
  }
  return false;
}

/** Every import specifier in code that `allowed` doesn't contain. */
export function unsupportedImports(source: string, allowed: (spec: string) => boolean): string[] {
  const out = new Set<string>();
  const ranges = codeRanges(source);
  for (const m of source.matchAll(importSpecifierRe())) {
    const spec = m[3];
    if (!spec || !inCode(ranges, m.index ?? 0)) continue;
    if (!allowed(spec)) out.add(spec);
  }
  return [...out];
}

/** Rewrite only in-code mapped specifiers to their shim URLs. */
export function rewriteSpecifiers(source: string, map: Record<string, string>): string {
  const ranges = codeRanges(source);
  return source.replace(importSpecifierRe(), (whole, pre, quote, spec, offset) =>
    map[spec] && inCode(ranges, Number(offset)) ? `${pre}${quote}${map[spec]}${quote}` : whole,
  );
}
