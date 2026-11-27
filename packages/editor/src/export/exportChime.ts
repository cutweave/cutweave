// SPDX-License-Identifier: GPL-3.0-or-later
// The sound of a finished export (WP-2G.04b, Settings > Notifications > Export sound): two short
// rising notes made with the Web Audio API in the editor's page, so the app ships no sound file
// and the engine's audio output is not involved. Quiet, half a second, and silent where the page
// has no audio (tests, a browser that blocks it).

/** The two notes, in Hz (A5, then D6), and when each starts, in seconds. */
const NOTES: readonly (readonly [frequency: number, start: number])[] = [
  [880, 0],
  [1174.66, 0.14],
];
const NOTE_SECONDS = 0.32;
const PEAK_GAIN = 0.12;

type AudioContextClass = typeof AudioContext;

/** Plays the chime once; does nothing when the page cannot play audio. */
export function playExportChime(
  Context: AudioContextClass | undefined = (globalThis as { AudioContext?: AudioContextClass })
    .AudioContext,
): void {
  if (Context === undefined) return;
  try {
    const context = new Context();
    const now = context.currentTime;
    for (const [frequency, start] of NOTES) {
      const tone = context.createOscillator();
      const level = context.createGain();
      tone.type = "sine";
      tone.frequency.value = frequency;
      // A soft attack and an exponential fall, so the note neither clicks nor rings on.
      level.gain.setValueAtTime(0.0001, now + start);
      level.gain.exponentialRampToValueAtTime(PEAK_GAIN, now + start + 0.02);
      level.gain.exponentialRampToValueAtTime(0.0001, now + start + NOTE_SECONDS);
      tone.connect(level);
      level.connect(context.destination);
      tone.start(now + start);
      tone.stop(now + start + NOTE_SECONDS);
      if (start === NOTES[NOTES.length - 1]?.[1]) {
        tone.onended = () => void context.close().catch(() => undefined);
      }
    }
  } catch {
    // No audio device, or the page may not play: the notification still shows.
  }
}
