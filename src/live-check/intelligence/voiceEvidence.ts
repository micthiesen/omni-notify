const VOICE_HIT_WINDOW_MS = 10 * 60_000;
const SPEECH_MISSES_TO_RESET = 5;

export type VoiceEvidenceDecision = "none" | "possible" | "confirmed";

export class VoiceEvidenceTracker {
  private readonly hits = new Map<string, number[]>();
  private readonly misses = new Map<string, number>();

  public observe(
    streamerId: string,
    matchedWindows: number,
    checkedWindows: number,
    now = Date.now(),
  ): VoiceEvidenceDecision {
    // Silence is not counter-evidence. DGG guests often speak intermittently,
    // and VAD can legitimately produce 0/0 between their turns.
    if (checkedWindows === 0) return "none";
    if (matchedWindows < 2) {
      const misses = (this.misses.get(streamerId) ?? 0) + 1;
      this.misses.set(streamerId, misses);
      if (misses >= SPEECH_MISSES_TO_RESET) this.hits.delete(streamerId);
      return "none";
    }
    this.misses.set(streamerId, 0);
    const hits = [...(this.hits.get(streamerId) ?? []), now].filter(
      (at) => at >= now - VOICE_HIT_WINDOW_MS,
    );
    this.hits.set(streamerId, hits);
    return hits.length >= 2 ? "confirmed" : "possible";
  }

  public clear(streamerId: string): void {
    this.hits.delete(streamerId);
    this.misses.delete(streamerId);
  }
}
