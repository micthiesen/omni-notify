import { describe, expect, it } from "vitest";
import { decide, eligibleQueueItem, observationFingerprint } from "./policy.js";
import type { Evidence, ImportFile, QueueItem, Target } from "./types.js";

function queue(overrides: Partial<QueueItem> = {}): QueueItem {
  return {
    id: 10,
    downloadId: "download-1",
    title: "House.of.Cards.US.S01E01.1080p.WEB-DL",
    status: "completed",
    trackedDownloadStatus: "warning",
    trackedDownloadState: "importPending",
    statusMessages: [
      { title: "Import failed", messages: ["Episode was not imported automatically"] },
    ],
    size: 1_000,
    sizeleft: 0,
    outputPath: "/downloads/House.of.Cards.US.S01E01",
    seriesId: 42,
    ...overrides,
  };
}

function target(overrides: Partial<Target> = {}): Target {
  return {
    id: 42,
    title: "House of Cards",
    year: 2013,
    monitored: true,
    hasFile: false,
    path: "/media/House of Cards",
    episodeIds: [101],
    episodes: [
      {
        id: 101,
        seasonNumber: 1,
        episodeNumber: 1,
        title: "Chapter 1",
        hasFile: false,
        monitored: true,
      },
    ],
    alternateTitles: ["House of Cards (US)"],
    ...overrides,
  };
}

function file(overrides: Partial<ImportFile> = {}): ImportFile {
  return {
    id: 501,
    path: "/downloads/House.of.Cards.US.S01E01/House.of.Cards.US.S01E01.mkv",
    name: "House.of.Cards.US.S01E01.mkv",
    size: 1_000,
    seriesId: 42,
    seasonNumber: 1,
    episodeIds: [101],
    quality: {},
    rejections: [],
    ...overrides,
  };
}

function evidence(overrides: Partial<Evidence> = {}): Evidence {
  return {
    kind: "sonarr",
    items: [queue()],
    target: target(),
    files: [file()],
    grabs: [
      {
        downloadId: "download-1",
        sourceTitle: "House.of.Cards.US.S01E01.1080p.WEB-DL",
        seriesId: 42,
        episodeId: 101,
        eventType: "grabbed",
        date: "2026-09-12T00:00:00Z",
      },
    ],
    ...overrides,
  };
}

describe("eligibleQueueItem", () => {
  it("accepts a completed, diagnosed import failure", () => {
    expect(eligibleQueueItem(queue())).toBe(true);
  });

  it.each(["downloading", "queued", "parCheck", "unpacking", "importing"])(
    "rejects active state %s",
    (trackedDownloadState) => {
      expect(eligibleQueueItem(queue({ trackedDownloadState }))).toBe(false);
    },
  );

  it("requires substantive diagnostics", () => {
    expect(
      eligibleQueueItem(queue({ statusMessages: [{ title: "", messages: [] }] })),
    ).toBe(false);
  });

  it("requires the completed download to have no bytes left", () => {
    expect(eligibleQueueItem(queue({ sizeleft: 1 }))).toBe(false);
  });

  it("admits an explicit terminal no-files failure for further corroboration", () => {
    expect(
      eligibleQueueItem(
        queue({
          status: "failed",
          trackedDownloadState: "failed",
          statusMessages: [
            { title: "No files found are eligible for import", messages: [] },
          ],
        }),
      ),
    ).toBe(true);
  });
});

describe("observationFingerprint", () => {
  it("is stable across queue and diagnostic ordering and ignores ephemeral queue IDs", () => {
    const first = queue({
      statusMessages: [
        { title: "Second", messages: ["B", "A"] },
        { title: "First", messages: ["C"] },
      ],
    });
    const second = queue({ id: 11, downloadId: "download-2" });
    const reorderedFirst = {
      ...first,
      id: 999,
      statusMessages: [
        { title: "First", messages: ["C"] },
        { title: "Second", messages: ["A", "B"] },
      ],
    };
    expect(observationFingerprint([first, second])).toBe(
      observationFingerprint([second, reorderedFirst]),
    );
  });

  it("changes when the failure state changes", () => {
    expect(observationFingerprint([queue()])).not.toBe(
      observationFingerprint([queue({ trackedDownloadState: "importBlocked" })]),
    );
  });
});

describe("decide", () => {
  it("imports an exact Sonarr mapping and supports the common US title suffix", () => {
    expect(decide(evidence())).toMatchObject({ action: "import", source: "rules" });
  });

  it("never acts on an active download", () => {
    const active = evidence({
      items: [
        queue({
          status: "downloading",
          trackedDownloadState: "downloading",
          sizeleft: 100,
        }),
      ],
      target: target({ episodes: [{ ...target().episodes[0]!, hasFile: true }] }),
      files: [
        file({
          rejections: [
            {
              type: "quality",
              reason: "Existing file on disk is of equal or higher quality",
            },
          ],
        }),
      ],
    });
    expect(decide(active).action).toBe("defer");
  });

  it("defers mismatched episode numbering", () => {
    expect(
      decide(evidence({ files: [file({ name: "House.of.Cards.US.S01E02.mkv" })] })),
    ).toEqual(expect.objectContaining({ action: "defer" }));
  });

  it("leaves an opaque filename for semantic assessment", () => {
    expect(
      decide(
        evidence({
          files: [
            file({
              name: "NqFGW2VSR2C49AkyiFgnB6G.mkv",
              path: "/downloads/House.of.Cards.US.S01E01/NqFGW2VSR2C49AkyiFgnB6G.mkv",
            }),
          ],
        }),
      ).action,
    ).toBe("defer");
  });

  it("does not collapse distinct regional aliases", () => {
    expect(
      decide(
        evidence({
          target: target({ title: "The Office (US)", alternateTitles: [] }),
          files: [
            file({
              name: "The.Office.UK.S01E01.mkv",
              path: "/downloads/House.of.Cards.US.S01E01/The.Office.UK.S01E01.mkv",
            }),
          ],
        }),
      ).action,
    ).toBe("defer");
  });

  it("rejects an explicit year from a different series incarnation", () => {
    expect(
      decide(
        evidence({
          target: target({ title: "Doctor Who", year: 2005, alternateTitles: [] }),
          files: [
            file({
              name: "Doctor.Who.1963.S01E01.mkv",
              path: "/downloads/House.of.Cards.US.S01E01/Doctor.Who.1963.S01E01.mkv",
            }),
          ],
        }),
      ).action,
    ).toBe("defer");
  });

  it("defers a file outside the download output path", () => {
    expect(decide(evidence({ files: [file({ path: "/tmp/injected.mkv" })] }))).toEqual(
      expect.objectContaining({ action: "defer" }),
    );
  });

  it("defers permission, sample, and corruption evidence", () => {
    for (const message of ["Permission denied", "File is a sample", "CRC corrupt"]) {
      const result = decide(
        evidence({
          items: [
            queue({
              statusMessages: [{ title: "Import failed", messages: [message] }],
            }),
          ],
        }),
      );
      expect(result.action).toBe("defer");
    }
  });

  it("removes without searching when every intended file exists and all rejections are downgrades", () => {
    const result = decide(
      evidence({
        target: target({
          episodes: [{ ...target().episodes[0]!, hasFile: true }],
        }),
        files: [
          file({
            rejections: [
              {
                type: "quality",
                reason: "Existing file on disk is of equal or higher quality",
              },
              {
                type: "customFormat",
                reason: "Not a Custom Format upgrade for existing episode file",
              },
            ],
          }),
        ],
      }),
    );
    expect(result).toMatchObject({ action: "remove", replace: false, source: "rules" });
  });

  it("does not remove a no-files failure without download-client health corroboration", () => {
    const noFiles = evidence({
      items: [
        queue({
          status: "failed",
          trackedDownloadState: "failed",
          statusMessages: [
            {
              title: "No files found are eligible for import in /tmp/inter",
              messages: [],
            },
          ],
        }),
      ],
      files: [],
    });
    expect(decide(noFiles).action).toBe("defer");
    expect(decide({ ...noFiles, downloadHealth: "WARNING/HEALTH" })).toMatchObject({
      action: "remove",
      replace: true,
    });
  });

  it("accepts a corroborated terminal unpack failure for replacement", () => {
    const noFiles = evidence({
      items: [
        queue({
          status: "failed",
          trackedDownloadState: "failed",
          statusMessages: [
            { title: "No files found are eligible for import", messages: [] },
          ],
        }),
      ],
      files: [],
    });
    expect(decide({ ...noFiles, downloadHealth: "FAILURE/UNPACK" })).toMatchObject({
      action: "remove",
      replace: true,
    });
    expect(decide({ ...noFiles, downloadHealth: "FAILURE/PASSWORD" }).action).toBe(
      "defer",
    );
  });

  it("cleans a redundant downgrade even when Arr could not determine whether it is a sample", () => {
    const result = decide(
      evidence({
        items: [
          queue({
            statusMessages: [
              {
                title: "Import failed",
                messages: ["Unable to determine if file is a sample"],
              },
            ],
          }),
        ],
        target: target({ episodes: [{ ...target().episodes[0]!, hasFile: true }] }),
        files: [
          file({
            rejections: [
              {
                type: "sample",
                reason: "Unable to determine if file is a sample",
              },
              {
                type: "customFormat",
                reason: "Not a Custom Format upgrade for existing episode file",
              },
            ],
          }),
        ],
      }),
    );
    expect(result).toMatchObject({ action: "remove", replace: false });
  });

  it("does not clean a downgrade when hard infrastructure evidence is also present", () => {
    const result = decide(
      evidence({
        items: [
          queue({
            statusMessages: [
              { title: "Import failed", messages: ["Permission denied"] },
            ],
          }),
        ],
        target: target({ episodes: [{ ...target().episodes[0]!, hasFile: true }] }),
        files: [
          file({
            rejections: [
              {
                type: "quality",
                reason: "Existing file on disk is of equal or higher quality",
              },
            ],
          }),
        ],
      }),
    );
    expect(result.action).toBe("defer");
  });
});

it("resets the failure fingerprint on size, target, or path changes", () => {
  const first = queue();
  for (const update of [
    { size: 2000 },
    { sizeleft: 100 },
    { episodeId: 999 },
    { outputPath: "/downloads/new" },
  ])
    expect(observationFingerprint([first])).not.toBe(
      observationFingerprint([{ ...first, ...update }]),
    );
});
it("does not mistake a longer title for the requested series", () => {
  expect(
    decide(
      evidence({
        target: target({ title: "Berserk", alternateTitles: [] }),
        files: [file({ name: "Berserk.of.Gluttony.S01E01.mkv" })],
      }),
    ).action,
  ).toBe("defer");
});
it("requires grabbed episode IDs, not just a matching series", () => {
  const e = evidence();
  e.grabs[0].episodeId = 999;
  expect(decide(e).action).toBe("defer");
});
