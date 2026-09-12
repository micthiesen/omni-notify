import { describe, expect, it } from "vitest";
import { validateLlmDecision } from "./llm.js";
import type { Evidence, ImportFile, QueueItem, Target } from "./types.js";

function queue(overrides: Partial<QueueItem> = {}): QueueItem {
  return {
    id: 10,
    downloadId: "download-1",
    title: "Ambiguous.Release.S01E01",
    status: "completed",
    trackedDownloadStatus: "warning",
    trackedDownloadState: "importBlocked",
    statusMessages: [
      {
        title: "Import failed",
        messages: ["Release could not be mapped automatically"],
      },
    ],
    size: 1_000,
    sizeleft: 0,
    outputPath: "/downloads/ambiguous",
    seriesId: 42,
    ...overrides,
  };
}

function target(overrides: Partial<Target> = {}): Target {
  return {
    id: 42,
    title: "The Example",
    year: 2020,
    monitored: true,
    hasFile: false,
    path: "/media/The Example",
    episodeIds: [101],
    episodes: [
      {
        id: 101,
        seasonNumber: 1,
        episodeNumber: 1,
        title: "Pilot",
        hasFile: false,
        monitored: true,
      },
    ],
    alternateTitles: [],
    ...overrides,
  };
}

function file(overrides: Partial<ImportFile> = {}): ImportFile {
  return {
    id: 501,
    path: "/downloads/ambiguous/NqFGW2VSR2C49AkyiFgnB6G.mkv",
    name: "NqFGW2VSR2C49AkyiFgnB6G.mkv",
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
        sourceTitle: "Alias.Name.S01E01",
        seriesId: 42,
        episodeId: 101,
        eventType: "grabbed",
        date: "2026-09-12T00:00:00Z",
      },
    ],
    ...overrides,
  };
}

function verdict(overrides: Record<string, unknown> = {}) {
  return {
    action: "import",
    diagnosis: "safe_import",
    reason: "The alternate release title and episode numbering describe the target",
    confidence: 0.95,
    replace: false,
    fileIds: [501],
    downloadIds: ["download-1"],
    ...overrides,
  };
}

describe("validateLlmDecision", () => {
  it("accepts a high-confidence opaque-filename import while preserving structural guards", () => {
    expect(validateLlmDecision(evidence(), verdict())).toMatchObject({
      action: "import",
      source: "llm",
    });
  });

  it.each([
    ["unknown file", { fileIds: [999] }],
    ["invented download", { downloadIds: ["download-2"] }],
    ["low confidence", { confidence: 0.89 }],
    ["wrong diagnosis", { diagnosis: "ambiguous" }],
    ["replacement import", { replace: true }],
  ])("defers an import with %s", (_name, override) => {
    expect(validateLlmDecision(evidence(), verdict(override)).action).toBe("defer");
  });

  it("defers an import with a rejection or a file outside the output path", () => {
    const rejected = evidence({
      files: [
        file({ rejections: [{ type: "quality", reason: "Unknown import rejection" }] }),
      ],
    });
    const escaped = evidence({ files: [file({ path: "/tmp/injected.mkv" })] });
    expect(validateLlmDecision(rejected, verdict()).action).toBe("defer");
    expect(validateLlmDecision(escaped, verdict()).action).toBe("defer");
  });

  it("never lets the model override infrastructure or media-integrity evidence", () => {
    for (const message of [
      "Permission denied",
      "Sample file detected",
      "Media is corrupt",
    ]) {
      const unsafe = evidence({
        items: [
          queue({ statusMessages: [{ title: "Import failed", messages: [message] }] }),
        ],
      });
      expect(validateLlmDecision(unsafe, verdict()).action).toBe("defer");
    }
  });

  it("allows replacement only for confidently wrong content with matching grab history", () => {
    const wrongContent = evidence({
      files: [file({ seriesId: 999, episodeIds: [], rejections: [] })],
    });
    expect(
      validateLlmDecision(
        wrongContent,
        verdict({
          action: "remove",
          diagnosis: "wrong_content",
          reason: "The file belongs to an unrelated series",
          replace: true,
        }),
      ),
    ).toMatchObject({ action: "remove", replace: true, source: "llm" });
  });

  it("defers replacement when it would discard a valid requested partial file", () => {
    expect(
      validateLlmDecision(
        evidence(),
        verdict({
          action: "remove",
          diagnosis: "wrong_content",
          replace: true,
        }),
      ).action,
    ).toBe("defer");
  });

  it("defers replacement without known matching grab history", () => {
    const wrongContent = evidence({
      files: [file({ seriesId: 999, episodeIds: [] })],
      grabs: [],
    });
    expect(
      validateLlmDecision(
        wrongContent,
        verdict({
          action: "remove",
          diagnosis: "wrong_content",
          replace: true,
        }),
      ).action,
    ).toBe("defer");
  });

  it("defers replacement of an active download or an empty preview", () => {
    const removalVerdict = verdict({
      action: "remove",
      diagnosis: "wrong_content",
      replace: true,
    });
    const active = evidence({
      items: [
        queue({
          status: "downloading",
          trackedDownloadState: "downloading",
          sizeleft: 500,
        }),
      ],
      files: [file({ seriesId: 999, episodeIds: [] })],
    });
    expect(validateLlmDecision(active, removalVerdict).action).toBe("defer");
    expect(
      validateLlmDecision(
        evidence({ files: [] }),
        verdict({
          ...removalVerdict,
          fileIds: [],
        }),
      ).action,
    ).toBe("defer");
  });

  it("returns a model defer reason without making it executable", () => {
    expect(
      validateLlmDecision(
        evidence(),
        verdict({
          action: "defer",
          diagnosis: "ambiguous",
          reason: "Numbering remains ambiguous",
          confidence: 0.5,
        }),
      ),
    ).toEqual({
      action: "defer",
      reason: "Numbering remains ambiguous",
      source: "llm",
    });
  });

  it("turns malformed output into a conservative defer", () => {
    expect(validateLlmDecision(evidence(), { action: "import" })).toEqual(
      expect.objectContaining({ action: "defer", source: "llm" }),
    );
  });
});
