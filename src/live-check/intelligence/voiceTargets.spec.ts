import { describe, expect, it } from "vitest";
import { selectVoiceTargets } from "./voiceTargets.js";

function target(id: string, viewers: number, hosted = false) {
  return { streamer: { id, dgg: { viewers, hosted } } };
}

describe("selectVoiceTargets", () => {
  it("selects the highest-viewed targets rather than network completion order", () => {
    const xqc = target("dgg:kick:xqc", 7);
    const prsek = target("prsek", 194);
    const pryingMind = target("dgg:kick:pryingmind", 381, true);
    const bingsamaa = target("dgg:twitch:bingsamaa", 69);

    expect(
      selectVoiceTargets([xqc, bingsamaa, pryingMind, prsek], 3).map(
        (item) => item.streamer.id,
      ),
    ).toEqual(["dgg:kick:pryingmind", "prsek", "dgg:twitch:bingsamaa"]);
  });

  it("uses hosted state and id as deterministic tie breakers", () => {
    expect(
      selectVoiceTargets(
        [target("z", 10), target("b", 10, true), target("a", 10)],
        3,
      ).map((item) => item.streamer.id),
    ).toEqual(["b", "a", "z"]);
  });
});
