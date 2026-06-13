// LaTeX a tutor answer slipped into, as text a student can read (dry run, 2026-06-13: answers showed
// `\(1/\sqrt{C2}\)` raw, at every thinking level). The answer is plain text by its prompt, and is
// rendered as text, never as HTML (LLD §14): this only rewrites the notation, into text with
// subscripts and superscripts. Applied to the text between reference chips, so their spans hold.

export type MathPart = { text: string } | { sub: string } | { sup: string };

const SYMBOLS: Record<string, string> = {
  times: "×", cdot: "·", approx: "≈", pi: "π", Omega: "Ω", omega: "ω", mu: "µ", pm: "±", le: "≤", leq: "≤",
  ge: "≥", geq: "≥", neq: "≠", ne: "≠", infty: "∞", Delta: "Δ", delta: "δ", tau: "τ", theta: "θ", phi: "φ",
  varphi: "φ", alpha: "α", beta: "β", lambda: "λ", circ: "°", degree: "°", parallel: "∥", to: "→",
  rightarrow: "→", Rightarrow: "⇒", propto: "∝", sim: "~", ldots: "…", dots: "…", cdots: "…", quad: "  ",
  qquad: "    ", left: "", right: "", displaystyle: "", big: "", Big: "",
};
const SPACES: Record<string, string> = { ",": " ", ";": " ", ":": " ", "!": "", " ": " ", "%": "%", "{": "{", "}": "}", _: "_" };

/** A group's text is simple when it needs no brackets around it: "2", "C2", "RC". */
const simple = (s: string) => /^√?[A-Za-z0-9.]{1,3}$/.test(s);
const group = (s: string) => (simple(s) ? s : `(${s})`);

/** The notation in `text` read as text, subscripts and superscripts; text without LaTeX is returned whole. */
export function mathText(text: string): MathPart[] {
  if (!text.includes("\\") && !text.includes("$")) return [{ text }];
  let s = text
    .replace(/\\[()[\]]/g, "") // \( \) \[ \]: math delimiters
    .replace(/\$([^$\n]*[\\^_=][^$\n]*)\$/g, "$1") // $…$ around maths (not a lone dollar sign)
    // Symbols first: `\mu\text{s}` must not become `\mus` once the text is unwrapped.
    .replace(/\\([A-Za-z]+)/g, (m, word: string) => SYMBOLS[word] ?? m);
  for (let i = 0; i < 3; i++) {
    // innermost first, for nested \frac and \sqrt
    s = s.replace(/\\(?:text|mathrm|operatorname|mathbf|mathit|textrm)\{([^{}]*)\}/g, "$1");
    s = s.replace(/\\[dt]?frac\{([^{}]*)\}\{([^{}]*)\}/g, (_, a: string, b: string) => `${group(a.trim())}/${group(b.trim())}`);
    s = s.replace(/\\sqrt\{([^{}]*)\}/g, (_, a: string) => `√${group(a.trim())}`);
  }
  s = s.replace(/\\([A-Za-z]+)|\\(.)/g, (_, word: string | undefined, char: string | undefined) =>
    word !== undefined ? (SYMBOLS[word] ?? word) : (SPACES[char!] ?? char!),
  );
  s = s.replace(/\^\{?°\}?/g, "°");
  return scripts(s);
}

/** `_{…}`, `_x`, `^{…}`, `^x` as subscripts and superscripts; other braces dropped. A single
 * character after `_` counts only when nothing alphanumeric follows it (B2_OUT stays a name). */
function scripts(s: string): MathPart[] {
  const out: MathPart[] = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ text: text.replace(/[{}]/g, "") });
    text = "";
  };
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!;
    if ((c === "_" || c === "^") && i > 0) {
      let inner: string | null = null;
      let end = i;
      if (s[i + 1] === "{") {
        const close = s.indexOf("}", i + 2);
        if (close > 0) [inner, end] = [s.slice(i + 2, close), close];
      } else if (/[A-Za-z0-9]/.test(s[i + 1] ?? "") && !/[A-Za-z0-9_]/.test(s[i + 2] ?? "")) {
        [inner, end] = [s[i + 1]!, i + 1];
      } else if (c === "^" && /[-−]/.test(s[i + 1] ?? "") && /\d/.test(s[i + 2] ?? "")) {
        const m = /^[-−]\d+/.exec(s.slice(i + 1))!;
        [inner, end] = [m[0], i + m[0].length];
      }
      if (inner !== null) {
        flush();
        out.push(c === "_" ? { sub: inner.replace(/[{}]/g, "") } : { sup: inner.replace(/[{}]/g, "") });
        i = end;
        continue;
      }
    }
    text += c;
  }
  flush();
  return out;
}

/** The notation read as plain text (sub- and superscripts inline), for a label or a title. */
export function mathPlain(text: string): string {
  return mathText(text)
    .map((p) => ("text" in p ? p.text : "sub" in p ? p.sub : `^${p.sup}`))
    .join("");
}
