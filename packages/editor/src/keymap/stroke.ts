// SPDX-License-Identifier: GPL-3.0-or-later
// Keys as data (WP-2H.05): a stroke is one key with its modifiers, a binding's keys are one stroke
// or a chord of two (`Mod+K Mod+S`). A stroke is spelled `Mod+Alt+Shift+<Key>`: `Mod` is Ctrl, or
// Cmd on macOS (the platform modifier mapping), and `Ctrl` is the physical Control key on macOS
// only. `<Key>` is a letter or digit in upper case, one punctuation character, or a name
// (`Space`, `Delete`, `ArrowLeft`, `F2`).
//
// Any keyboard layout: a letter is matched by what the key prints (`KeyboardEvent.key`) while that
// is a Latin letter, and by the physical key (`KeyboardEvent.code`) when the layout prints
// something else (Hebrew, Russian), as the native menu's accelerators do. Record-a-shortcut
// stores the same spelling, so a shortcut recorded on one layout works on another.

/** Where the shortcuts run: the platform decides what `Mod` means and a few keys. */
export type KeyPlatform = "windows" | "linux" | "mac";

/** One key with its modifiers; `next` is the second stroke of a chord. */
export interface KeyCombo {
  /** A letter in upper case, a digit, one punctuation character, or a name (`Space`, `F2`). */
  readonly key: string;
  /** Ctrl, or Cmd on macOS. */
  readonly mod?: boolean;
  /** The physical Control key on macOS (Ctrl and Mod are the same key elsewhere). */
  readonly ctrl?: boolean;
  readonly alt?: boolean;
  readonly shift?: boolean;
  readonly next?: KeyCombo;
}

/** The slice of a `KeyboardEvent` the keymap reads. */
export interface KeyEventLike {
  readonly key: string;
  readonly code?: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly repeat?: boolean;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
}

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "AltGraph", "OS", "CapsLock"]);
const NO_KEY = new Set(["Dead", "Process", "Unidentified", "Compose"]);

/** Punctuation by physical key: the layout-independent name of a key that prints a symbol. */
const CODE_PUNCTUATION: Readonly<Record<string, string>> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

/** The letter, digit or punctuation a physical key stands for on a US layout, or `null`. */
export function keyFromCode(code: string | undefined): string | null {
  if (code === undefined) return null;
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter !== null) return letter[1] ?? null;
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit !== null) return digit[1] ?? null;
  return CODE_PUNCTUATION[code] ?? null;
}

/** The physical key (`KeyboardEvent.code`) of a spelled key, to show it in the person's layout. */
export function codeOfKey(key: string): string | null {
  if (/^[A-Z]$/.test(key)) return `Key${key}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  for (const [code, char] of Object.entries(CODE_PUNCTUATION)) if (char === key) return code;
  return null;
}

/** The main key of an event as the spelling names it, or `null` for a modifier or a dead key. */
export function mainKeyOf(e: Pick<KeyEventLike, "key" | "code" | "shiftKey">): string | null {
  const { key } = e;
  if (MODIFIER_KEYS.has(key) || NO_KEY.has(key) || key === "") return null;
  if (key === " ") return "Space";
  if (key === "Esc") return "Escape";
  if (key === "Spacebar") return "Space";
  if (key.length > 1 && !/^\p{L}$/u.test(key)) return key;
  const ascii = /^[\x20-\x7e]$/.test(key);
  if (!ascii) {
    // The layout prints something else (Hebrew, Russian, a macOS Option letter): the physical key.
    return keyFromCode(e.code) ?? key.toUpperCase();
  }
  if (/^[a-zA-Z]$/.test(key)) return key.toUpperCase();
  // Digits and symbols: with Shift held the character is the shifted one (`!`), so use the key.
  if (e.shiftKey) return keyFromCode(e.code) ?? key;
  return key;
}

/** The stroke an event is, on `platform`; `null` for a bare modifier or a key with no name. */
export function strokeOfEvent(e: KeyEventLike, platform: KeyPlatform): KeyCombo | null {
  const key = mainKeyOf(e);
  if (key === null) return null;
  if (platform === "mac") {
    return {
      key,
      ...(e.metaKey && { mod: true }),
      ...(e.ctrlKey && { ctrl: true }),
      ...(e.altKey && { alt: true }),
      ...(e.shiftKey && { shift: true }),
    };
  }
  // The Windows or Super key belongs to the OS: no stroke carries it.
  if (e.metaKey) return null;
  return {
    key,
    ...(e.ctrlKey && { mod: true }),
    ...(e.altKey && { alt: true }),
    ...(e.shiftKey && { shift: true }),
  };
}

/** A stroke as its spelling: `Mod+Shift+Z`. A chord adds a space and the second stroke. */
export function spell(combo: KeyCombo): string {
  const parts = [
    ...(combo.mod === true ? ["Mod"] : []),
    ...(combo.ctrl === true ? ["Ctrl"] : []),
    ...(combo.alt === true ? ["Alt"] : []),
    ...(combo.shift === true ? ["Shift"] : []),
    combo.key,
  ];
  const first = parts.join("+");
  return combo.next === undefined ? first : `${first} ${spell(combo.next)}`;
}

function parseStroke(text: string, platform: KeyPlatform): KeyCombo | null {
  const tokens = text.split("+");
  let mod = false;
  let ctrl = false;
  let alt = false;
  let shift = false;
  let i = 0;
  // Modifiers first; whatever remains (a `+` included) is the key.
  while (i < tokens.length - 1) {
    const token = tokens[i];
    if (token === "Mod" || token === "Cmd" || token === "Command") mod = true;
    else if (token === "Ctrl" || token === "Control") {
      if (platform === "mac") ctrl = true;
      else mod = true;
    } else if (token === "Alt" || token === "Option") alt = true;
    else if (token === "Shift") shift = true;
    else break;
    i += 1;
  }
  const rest = tokens.slice(i).join("+");
  if (rest === "") return null;
  const key = rest.length === 1 ? rest.toUpperCase() : rest;
  return {
    key,
    ...(mod && { mod: true }),
    ...(ctrl && { ctrl: true }),
    ...(alt && { alt: true }),
    ...(shift && { shift: true }),
  };
}

/** The keys of a spelling (a stroke or a chord), or `null` when it is not one. */
export function parseKeys(text: string, platform: KeyPlatform): KeyCombo | null {
  const parts = text.trim().split(/ +/);
  if (parts.length === 0 || parts.length > 2) return null;
  const first = parseStroke(parts[0] ?? "", platform);
  if (first === null) return null;
  if (parts.length === 1) return first;
  const second = parseStroke(parts[1] ?? "", platform);
  return second === null ? null : { ...first, next: second };
}

/** Whether two spellings are the same stroke (chord) once parsed. */
export function sameKeys(a: KeyCombo, b: KeyCombo): boolean {
  return spell(a) === spell(b);
}

/** The first stroke of a binding's keys, without its chord tail. */
export function firstStroke(combo: KeyCombo): KeyCombo {
  const { next: _next, ...first } = combo;
  return first;
}

/** The spelling of a stroke alone (a chord's first one). */
export function strokeKey(combo: KeyCombo): string {
  return spell(firstStroke(combo));
}

// ---- how keys read ----------------------------------------------------------------------------

const KEY_NAMES: Readonly<Record<string, string>> = {
  Space: "Space",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Escape: "Esc",
};

/** Letters and digits as the person's own layout prints them (`navigator.keyboard`), by code. */
let layoutLabels: ReadonlyMap<string, string> | null = null;

/** Tells the labels what the person's layout prints on each physical key (or `null`: Latin). */
export function setLayoutLabels(map: ReadonlyMap<string, string> | null): void {
  layoutLabels = map;
}

/** Loads the layout where the platform offers it (Chromium's Keyboard Map API); never throws. */
export async function loadLayoutLabels(): Promise<void> {
  try {
    const keyboard = (
      navigator as unknown as {
        keyboard?: { getLayoutMap?: () => Promise<ReadonlyMap<string, string>> };
      }
    ).keyboard;
    const map = await keyboard?.getLayoutMap?.();
    if (map !== undefined) setLayoutLabels(map);
  } catch {
    // No layout map (the Keyboard Map API is unavailable): the keys read in Latin.
  }
}

function keyCap(key: string): string {
  if (key === " ") return "Space";
  const named = KEY_NAMES[key];
  if (named !== undefined) return named;
  if (key.length !== 1) return key;
  const code = codeOfKey(key);
  const printed = code === null ? undefined : layoutLabels?.get(code);
  return (printed ?? key).toUpperCase();
}

/** The parts of one stroke as key caps: `["Ctrl", "Shift", "S"]`, or `["⇧", "⌘", "S"]` on macOS. */
export function strokeParts(combo: KeyCombo, platform: KeyPlatform): string[] {
  const mac = platform === "mac";
  return [
    ...(combo.ctrl === true ? [mac ? "⌃" : "Ctrl"] : []),
    ...(combo.mod === true ? [mac ? "⌘" : "Ctrl"] : []),
    ...(combo.alt === true ? [mac ? "⌥" : "Alt"] : []),
    ...(combo.shift === true ? [mac ? "⇧" : "Shift"] : []),
    keyCap(combo.key),
  ];
}
