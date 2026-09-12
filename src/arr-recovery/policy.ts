import { createHash } from "node:crypto";
import path from "node:path";
import type {
  Decision,
  Evidence,
  Grab,
  ImportFile,
  QueueItem,
  Target,
} from "./types.js";

const IMPORT_FAILURE_STATES = new Set(["importblocked", "importpending"]);
const TRACKED_FAILURE_STATUSES = new Set(["warning", "error"]);
const ACTIVE_STATES = new Set([
  "downloading",
  "queued",
  "paused",
  "parcheck",
  "parrepair",
  "repairing",
  "unpacking",
  "importing",
]);

const UNSAFE_FAILURE =
  /\b(permission|access denied|unauthori[sz]ed|read[- ]?only|disk (?:full|space)|no space|filesystem|input\/output|i\/o error|sample|corrupt|damaged|crc|checksum|unpack(?:ing)? failed|invalid (?:video|media)|cannot (?:read|open)|failed to (?:read|open)|path does not exist)\b/i;
const HARD_UNSAFE_FAILURE =
  /\b(permission|access denied|unauthori[sz]ed|read[- ]?only|disk (?:full|space)|no space|filesystem|input\/output|i\/o error|corrupt|damaged|crc|checksum|unpack(?:ing)? failed|invalid (?:video|media)|cannot (?:read|open)|failed to (?:read|open)|path does not exist)\b/i;
const NO_ELIGIBLE_FILES = /\bno files? (?:found (?:are )?)?eligible for import\b/i;
const SAMPLE_REJECTION = /\bsample\b/i;
const QUALITY_REJECTION =
  /\b(existing|current|on disk)\b.*\b(?:equal|higher|better|upgrade|cutoff|quality|custom format|score)\b|\bnot (?:an? )?(?:quality|custom format|custom-format|cf)? ?upgrade\b|\bdoes not improve\b.*\b(?:quality|custom format|score)\b/i;

function normalizedEnum(value: string): string {
  return value.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "");
}

function statusText(item: QueueItem): string[] {
  return item.statusMessages.flatMap(({ title, messages }) => [title, ...messages]);
}

function substantiveMessages(item: QueueItem): string[] {
  return item.statusMessages
    .flatMap(({ messages }) => messages)
    .map((message) => message.trim())
    .filter((message) => message.length >= 5);
}

function isNormalImportFailure(item: QueueItem): boolean {
  return (
    normalizedEnum(item.status) === "completed" &&
    item.sizeleft === 0 &&
    TRACKED_FAILURE_STATUSES.has(normalizedEnum(item.trackedDownloadStatus)) &&
    IMPORT_FAILURE_STATES.has(normalizedEnum(item.trackedDownloadState)) &&
    substantiveMessages(item).length > 0
  );
}

function downloadHealth(evidence: Evidence): string {
  return evidence.downloadHealth ? normalizedEnum(evidence.downloadHealth) : "";
}

function isTerminalNoFilesFailure(evidence: Evidence): boolean {
  const unhealthyClient = new Set([
    "warninghealth",
    "failurehealth",
    "failurepar",
    "failureunpack",
  ]).has(downloadHealth(evidence));
  return (
    unhealthyClient &&
    evidence.items.length > 0 &&
    evidence.items.every(
      (item) =>
        ["completed", "failed", "warning"].includes(normalizedEnum(item.status)) &&
        TRACKED_FAILURE_STATUSES.has(normalizedEnum(item.trackedDownloadStatus)) &&
        statusText(item).some((message) => NO_ELIGIBLE_FILES.test(message)),
    )
  );
}

function isTerminalNoFilesQueueItem(item: QueueItem): boolean {
  return (
    item.sizeleft === 0 &&
    ["completed", "failed", "warning"].includes(normalizedEnum(item.status)) &&
    TRACKED_FAILURE_STATUSES.has(normalizedEnum(item.trackedDownloadStatus)) &&
    ["importblocked", "importpending", "failedpending", "failed"].includes(
      normalizedEnum(item.trackedDownloadState),
    ) &&
    statusText(item).some((message) => NO_ELIGIBLE_FILES.test(message))
  );
}

/** Only settled queue failures with explicit Arr diagnostics enter recovery. */
export function eligibleQueueItem(item: QueueItem): boolean {
  const state = normalizedEnum(item.trackedDownloadState);
  const status = normalizedEnum(item.status);
  if (ACTIVE_STATES.has(state) || ACTIVE_STATES.has(status)) return false;
  return isNormalImportFailure(item) || isTerminalNoFilesQueueItem(item);
}

/** A stable identity for one observed queue failure, independent of queue ordering. */
export function observationFingerprint(items: QueueItem[]): string {
  const failures = items
    .map((item) => ({
      downloadId: item.downloadId,
      title: item.title,
      outputPath: item.outputPath,
      seriesId: item.seriesId,
      episodeId: item.episodeId,
      movieId: item.movieId,
      size: item.size,
      sizeleft: item.sizeleft,
      added: item.added,
      status: normalizedEnum(item.status),
      trackedDownloadStatus: normalizedEnum(item.trackedDownloadStatus),
      trackedDownloadState: normalizedEnum(item.trackedDownloadState),
      messages: item.statusMessages
        .map(({ title, messages }) => ({
          title: title.trim(),
          messages: messages
            .map((message) => message.trim())
            .filter(Boolean)
            .sort(),
        }))
        .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
    }))
    .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
  return createHash("sha256").update(JSON.stringify(failures)).digest("hex");
}

function targetIdMatches(kind: Evidence["kind"], value: Grab | ImportFile, id: number) {
  return kind === "sonarr" ? value.seriesId === id : value.movieId === id;
}

export function hasMatchingGrabHistory(evidence: Evidence): boolean {
  const downloadIds = new Set(evidence.items.map((item) => item.downloadId));
  if (downloadIds.size === 0) return false;
  return [...downloadIds].every((downloadId) => {
    const grabs = evidence.grabs.filter(
      (grab) =>
        grab.downloadId === downloadId &&
        normalizedEnum(grab.eventType) === "grabbed" &&
        targetIdMatches(evidence.kind, grab, evidence.target.id),
    );
    return (
      grabs.length > 0 &&
      (evidence.kind === "radarr" ||
        evidence.target.episodeIds.every((id) =>
          grabs.some((grab) => grab.episodeId === id),
        ))
    );
  });
}

function intendedEpisodeIds(target: Target): Set<number> {
  return new Set(target.episodeIds);
}

function exactTargetMapping(evidence: Evidence): boolean {
  if (evidence.files.length === 0) return false;
  if (evidence.kind === "radarr") {
    return evidence.files.every(
      (file) => file.movieId === evidence.target.id && file.episodeIds.length === 0,
    );
  }

  const intended = intendedEpisodeIds(evidence.target);
  if (intended.size === 0) return false;
  const mapped = new Set<number>();
  for (const file of evidence.files) {
    if (file.seriesId !== evidence.target.id || file.episodeIds.length === 0)
      return false;
    for (const episodeId of file.episodeIds) {
      if (!intended.has(episodeId) || mapped.has(episodeId)) return false;
      mapped.add(episodeId);
    }
  }
  return mapped.size === intended.size;
}

function normalizeTitle(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase()
    .replace(/&/g, " and ")
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .replace(/\s+/g, " ");
}

function titleCandidates(target: Target): string[] {
  return [target.title, ...target.alternateTitles]
    .map(normalizeTitle)
    .filter(
      (title, index, titles) => title.length > 1 && titles.indexOf(title) === index,
    );
}

function filenameMatchesTitle(file: ImportFile, target: Target): boolean {
  const filename = path.basename(file.name || file.path);
  const marker =
    file.episodeIds.length > 0
      ? /s\d{1,2}[ ._-]*e\d{1,3}/i.exec(filename)
      : new RegExp(`(?:^|[^0-9])${target.year}(?=[^0-9]|$)`).exec(filename);
  if (!marker || marker.index === undefined) return false;
  const prefix = normalizeTitle(filename.slice(0, marker.index));
  return titleCandidates(target).some(
    (candidate) => prefix === candidate || prefix === `${candidate} ${target.year}`,
  );
}

function episodeKeysFromFilename(filename: string): Set<string> {
  const keys = new Set<string>();
  const expression = /s(\d{1,2})[ ._-]*e(\d{1,3})(?:[ ._-]*e(\d{1,3}))?/gi;
  for (const match of filename.matchAll(expression)) {
    const season = Number(match[1]);
    keys.add(`${season}:${Number(match[2])}`);
    if (match[3] !== undefined) keys.add(`${season}:${Number(match[3])}`);
  }
  return keys;
}

function filenameMatchesTarget(evidence: Evidence, file: ImportFile): boolean {
  if (!filenameMatchesTitle(file, evidence.target)) return false;
  const filename = path.basename(file.name || file.path);
  const years = [...filename.matchAll(/(?:^|\D)((?:19|20)\d{2})(?:\D|$)/g)].map(
    (match) => Number(match[1]),
  );
  if (
    years.length > 0 &&
    evidence.target.year > 0 &&
    !years.includes(evidence.target.year)
  ) {
    return false;
  }
  if (evidence.kind === "radarr") {
    if (evidence.target.year <= 0) return true;
    return years.includes(evidence.target.year);
  }

  const expected = new Set(
    file.episodeIds.flatMap((id) => {
      const episode = evidence.target.episodes.find((candidate) => candidate.id === id);
      return episode ? [`${episode.seasonNumber}:${episode.episodeNumber}`] : [];
    }),
  );
  const observed = episodeKeysFromFilename(filename);
  return (
    expected.size === file.episodeIds.length &&
    observed.size === expected.size &&
    [...expected].every((key) => observed.has(key))
  );
}

function fileIsConfinedToOutput(file: ImportFile, items: QueueItem[]): boolean {
  if (!path.isAbsolute(file.path)) return false;
  const filePath = path.resolve(file.path);
  return items.some((item) => {
    if (!item.outputPath || !path.isAbsolute(item.outputPath)) return false;
    const outputPath = path.resolve(item.outputPath);
    const relative = path.relative(outputPath, filePath);
    return (
      relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))
    );
  });
}

export function hasUnsafeFailureEvidence(evidence: Evidence): boolean {
  return [
    ...evidence.items.flatMap(statusText),
    ...evidence.files.flatMap((file) => file.rejections.map(({ reason }) => reason)),
  ].some((message) => UNSAFE_FAILURE.test(message) && !NO_ELIGIBLE_FILES.test(message));
}

export function canSafelyImport(evidence: Evidence): boolean {
  return (
    canStructurallyImport(evidence) &&
    evidence.files.every((file) => filenameMatchesTarget(evidence, file))
  );
}

/** Non-semantic invariants that an LLM verdict is never allowed to override. */
export function canStructurallyImport(evidence: Evidence): boolean {
  return (
    evidence.items.length > 0 &&
    evidence.items.every(isNormalImportFailure) &&
    hasMatchingGrabHistory(evidence) &&
    exactTargetMapping(evidence) &&
    evidence.files.every(
      (file) =>
        file.rejections.length === 0 && fileIsConfinedToOutput(file, evidence.items),
    ) &&
    !hasUnsafeFailureEvidence(evidence)
  );
}

export function hasNeededValidPartialFile(evidence: Evidence): boolean {
  return evidence.files.some(
    (file) =>
      file.rejections.length === 0 &&
      targetIdMatches(evidence.kind, file, evidence.target.id) &&
      (evidence.kind === "radarr" ||
        (file.episodeIds.length > 0 &&
          file.episodeIds.every((id) => intendedEpisodeIds(evidence.target).has(id)))),
  );
}

function allIntendedFilesAlreadyExist(evidence: Evidence): boolean {
  if (evidence.kind === "radarr") return evidence.target.hasFile;
  const intended = intendedEpisodeIds(evidence.target);
  return (
    intended.size > 0 &&
    [...intended].every(
      (id) =>
        evidence.target.episodes.find((episode) => episode.id === id)?.hasFile === true,
    )
  );
}

function onlyQualityDowngradeRejections(evidence: Evidence): boolean {
  return (
    evidence.files.length > 0 &&
    evidence.files.every(
      (file) =>
        file.rejections.length > 0 &&
        file.rejections.some(({ reason }) => QUALITY_REJECTION.test(reason)) &&
        file.rejections.every(
          ({ reason }) =>
            QUALITY_REJECTION.test(reason) || SAMPLE_REJECTION.test(reason),
        ),
    ) &&
    ![
      ...evidence.items.flatMap(statusText),
      ...evidence.files.flatMap((file) => file.rejections.map(({ reason }) => reason)),
    ].some((message) => HARD_UNSAFE_FAILURE.test(message))
  );
}

/** Conservative deterministic action. Ambiguity is left for the guarded LLM fallback. */
export function decide(evidence: Evidence): Decision {
  if (
    evidence.items.length === 0 ||
    evidence.items.some((item) => !eligibleQueueItem(item))
  ) {
    return {
      action: "defer",
      reason: "Download is not a settled, diagnosed import failure",
      source: "rules",
    };
  }
  if (
    hasMatchingGrabHistory(evidence) &&
    exactTargetMapping(evidence) &&
    allIntendedFilesAlreadyExist(evidence) &&
    onlyQualityDowngradeRejections(evidence)
  ) {
    return {
      action: "remove",
      reason:
        "Every intended item already has a file and the download is only a downgrade",
      source: "rules",
      replace: false,
    };
  }
  if (hasUnsafeFailureEvidence(evidence)) {
    return {
      action: "defer",
      reason:
        "Failure evidence may indicate an infrastructure or media-integrity problem",
      source: "rules",
    };
  }
  if (canSafelyImport(evidence)) {
    return {
      action: "import",
      reason:
        "Completed download has an exact, rejection-free target and filename mapping",
      source: "rules",
    };
  }
  if (
    isTerminalNoFilesFailure(evidence) &&
    hasMatchingGrabHistory(evidence) &&
    evidence.files.length === 0
  ) {
    return {
      action: "remove",
      reason:
        "Download client confirms a terminal failure with no files available to import",
      source: "rules",
      replace: true,
    };
  }
  return {
    action: "defer",
    reason: "Evidence does not support a safe deterministic recovery action",
    source: "rules",
  };
}
