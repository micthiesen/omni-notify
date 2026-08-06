/**
 * Coalesces rapid-fire title edits into one notification: the first change
 * notifies immediately, subsequent changes within the cooldown are held
 * (last-one-wins), and the held title fires once the cooldown lapses
 * (restarting the cooldown from that fire).
 *
 * State is in-memory only, per streamer. A process restart mid-live-session
 * loses it — the next title change after a restart just notifies immediately
 * (the same as a streamer with no prior state), which is an acceptable cold
 * start rather than something worth persisting.
 */

export const TITLE_CHANGE_COOLDOWN_MS = 10 * 60_000;

type DebounceState = {
  lastNotifiedAt: number;
  lastNotifiedTitle: string;
  pendingTitle?: string;
};

export type DebounceAction = { action: "notify"; title: string } | { action: "none" };

export class TitleChangeDebouncer {
  private states = new Map<string, DebounceState>();

  /**
   * Called on went-live. The go-live Pushover notification already carries
   * the title, so it counts as the last-notified title/time: a quick title
   * fix right after going live is held for the remainder of the cooldown
   * rather than notified immediately — that's the main spam case this exists
   * to prevent.
   */
  seed(streamerId: string, title: string, now: number): void {
    this.states.set(streamerId, { lastNotifiedAt: now, lastNotifiedTitle: title });
  }

  /**
   * Called on every still-live tick, not just when the title changed — a
   * held title needs a chance to fire on a later tick once its cooldown
   * expires even if nothing changed on that particular tick.
   */
  observe(
    streamerId: string,
    args: { currentTitle: string; titleChanged: boolean; now: number },
  ): DebounceAction {
    const { currentTitle, titleChanged, now } = args;
    const state = this.states.get(streamerId);

    // No seeded baseline (restart cold-start case, see class doc): treat a
    // change as the new baseline and notify immediately.
    if (!state) {
      if (!titleChanged) return { action: "none" };
      this.states.set(streamerId, {
        lastNotifiedAt: now,
        lastNotifiedTitle: currentTitle,
      });
      return { action: "notify", title: currentTitle };
    }

    const cooldownElapsed = now - state.lastNotifiedAt >= TITLE_CHANGE_COOLDOWN_MS;

    if (titleChanged) {
      if (cooldownElapsed) {
        state.lastNotifiedAt = now;
        state.lastNotifiedTitle = currentTitle;
        state.pendingTitle = undefined;
        return { action: "notify", title: currentTitle };
      }
      // Overwrite whatever was pending — last-one-wins within the cooldown.
      state.pendingTitle = currentTitle;
      return { action: "none" };
    }

    if (state.pendingTitle !== undefined && cooldownElapsed) {
      const pending = state.pendingTitle;
      state.pendingTitle = undefined;
      // A→B→A round trip: the held title already matches what was last
      // notified, so there's nothing new to announce.
      if (pending === state.lastNotifiedTitle) return { action: "none" };
      state.lastNotifiedAt = now;
      state.lastNotifiedTitle = pending;
      return { action: "notify", title: pending };
    }

    return { action: "none" };
  }

  /**
   * Called on went-offline AND on a primary-binding switch. decideTransition
   * never sets titleChanged on a primary switch, but a pending title held
   * from the old primary must not survive to be notified under the new one.
   */
  clear(streamerId: string): void {
    this.states.delete(streamerId);
  }
}
