import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  type PrinterStatus as IppPrinterStatus,
  Printer,
  type PrintJob,
  type PrintJobOptions,
} from "@pnosolutions/ipp";
import { getAttributes, IppOperation, IppTag } from "@pnosolutions/ipp-core";
import { Entity } from "@micthiesen/mitools/entities";
import { Data, Duration, Effect, Schedule, Schema } from "effect";
import { runPromise } from "../effect/interop.js";
import { publicGot } from "../press-pods/publicHttp.js";

export const PRINTER_DOWNLOAD_USER_AGENT =
  "OpenAI File Downloader, XaiImageApiFetch/1.0";
const MAX_PDF_BYTES = 20 * 1024 * 1024;
const MAX_PRINT_DATA_BYTES = 256 * 1024 * 1024;
const MAX_PAGES = 25;
const DUPLICATE_WINDOW_MS = 5 * 60 * 1000;
const JOB_STATUS_POLL_MS = 500;
const JOB_STATUS_MAX_POLLS = 120;
const CUPS_FILTER = "/usr/sbin/cupsfilter";
const BRLASER_FILTER = "/usr/lib/cups/filter/rastertobrlaser";
const BRLASER_PPD = "/usr/share/omni-printing/brother-hll2370dw.ppd";

export type PrintPaper = "letter" | "a4" | "legal";
export type PrintSides = "one-sided" | "two-sided-long-edge" | "two-sided-short-edge";
export interface PrintPdfInput {
  url: string;
  paper?: PrintPaper;
  sides?: PrintSides;
  copies?: number;
  jobName?: string;
  allowDuplicate?: boolean;
}
export interface PrinterStatus extends Record<string, unknown> {
  configured: true;
  name: string | null;
  uri: string;
  state: IppPrinterStatus["state"];
  stateReasons: string[];
  ready: boolean;
  acceptingJobs: boolean | null;
  queuedJobCount: number | null;
  tonerPercent: number | null;
  monochromeOnly: true;
  defaultSides: "two-sided-long-edge";
  supportedFormats: string[];
  supportedMedia: string[];
}
export interface AcceptedPrintJob extends Record<string, unknown> {
  accepted: true;
  completed: boolean;
  jobId: number | null;
  jobUri: string;
  jobState: PrintJob["state"];
  jobName: string;
  pages: number;
  copies: number;
  paper: PrintPaper;
  sides: PrintSides;
  impressionsCompleted: number | null;
  message: string;
}
/** Promise facade required by the MCP interface. */
export interface PrinterService {
  status(): Promise<PrinterStatus>;
  printPdf(input: PrintPdfInput): Promise<AcceptedPrintJob>;
}
export class PrinterError extends Data.TaggedError("PrinterError")<{
  operation: string;
  message: string;
  cause?: unknown;
}> {}

interface DownloadedFile {
  body: Uint8Array;
  contentType: string | undefined;
}
interface PrinterJobStatus {
  state: PrintJob["state"];
  stateReasons: string[];
  impressionsCompleted: number | null;
}
export interface AcceptedPrintRecord {
  fingerprint: string;
  acceptedAt: number;
  jobId: number | null;
  jobUri: string;
  jobState: PrintJob["state"];
  jobName: string;
}
export interface AcceptedPrintStore {
  get(fingerprint: string): AcceptedPrintRecord | undefined;
  upsert(record: AcceptedPrintRecord): void;
  deleteOlderThan(cutoff: number): void;
}
const PrinterAcceptedJobEntity = new Entity<AcceptedPrintRecord, ["fingerprint"]>(
  "printer-accepted-job",
  ["fingerprint"],
);
const durableAcceptedPrintStore: AcceptedPrintStore = {
  get: (fingerprint) => PrinterAcceptedJobEntity.get({ fingerprint }) ?? undefined,
  upsert: (record) => PrinterAcceptedJobEntity.upsert(record),
  deleteOlderThan: (cutoff) => {
    for (const record of PrinterAcceptedJobEntity.getAll()) {
      if (record.acceptedAt <= cutoff) {
        PrinterAcceptedJobEntity.delete({ fingerprint: record.fingerprint });
      }
    }
  },
};
interface PrinterClient {
  status(): Promise<IppPrinterStatus & { raw?: Record<string, unknown> }>;
  print(data: Uint8Array, options?: PrintJobOptions): Promise<PrintJob>;
  jobStatus?(jobUri: string): Promise<PrinterJobStatus>;
}
type ProcessRunner = (
  executable: string,
  args: string[],
) => Promise<{ stdout: string; stderr: string }>;
type BinaryProcessRunner = (
  executable: string,
  args: string[],
  environment?: Record<string, string>,
) => Promise<Buffer>;
export interface PrinterServiceDependencies {
  download?: (url: string) => Promise<DownloadedFile>;
  execFile?: ProcessRunner;
  execFileBuffer?: BinaryProcessRunner;
  printer?: PrinterClient;
  now?: () => number;
  wait?: (milliseconds: number) => Promise<void>;
  maxPrintDataBytes?: number;
  tempRoot?: string;
  acceptedPrintStore?: AcceptedPrintStore;
}
export interface PrinterServiceOptions {
  printerUrl?: string;
  requestTimeoutMs?: number;
  printerTimeoutMs?: number;
  dependencies?: PrinterServiceDependencies;
}
interface NormalizedInput {
  paper: PrintPaper;
  sides: PrintSides;
  copies: number;
  jobName: string;
}

const PrintInputSchema = Schema.Struct({
  url: Schema.String,
  paper: Schema.optional(Schema.Literals(["letter", "a4", "legal"])),
  sides: Schema.optional(
    Schema.Literals(["one-sided", "two-sided-long-edge", "two-sided-short-edge"]),
  ),
  copies: Schema.optional(Schema.Number),
  jobName: Schema.optional(Schema.String),
  allowDuplicate: Schema.optional(Schema.Boolean),
});
const mediaNames: Record<PrintPaper, string> = {
  letter: "na_letter_8.5x11in",
  a4: "iso_a4_210x297mm",
  legal: "na_legal_8.5x14in",
};
const cupsPaperNames: Record<PrintPaper, string> = {
  letter: "Letter",
  a4: "A4",
  legal: "Legal",
};
const cupsDuplexNames: Record<PrintSides, string> = {
  "one-sided": "None",
  "two-sided-long-edge": "DuplexNoTumble",
  "two-sided-short-edge": "DuplexTumble",
};
const failure = (operation: string, cause: unknown, message?: string) =>
  new PrinterError({
    operation,
    cause,
    message: message ?? (cause instanceof Error ? cause.message : String(cause)),
  });
const promiseCall = <A>(operation: string, evaluate: () => PromiseLike<A>) =>
  Effect.tryPromise({
    try: evaluate,
    catch: (cause) => failure(operation, cause),
  });

export function createPdfDownloaderEffect(
  requestTimeoutMs: number,
  requestPublicUrl: typeof publicGot = publicGot,
) {
  return (url: string): Effect.Effect<DownloadedFile, PrinterError> =>
    Effect.tryPromise({
      try: async (signal) => {
        const controller = new AbortController();
        const interrupt = () => controller.abort(signal.reason);
        signal.addEventListener("abort", interrupt, { once: true });
        try {
          const request = requestPublicUrl(url, {
            headers: {
              "User-Agent": PRINTER_DOWNLOAD_USER_AGENT,
              Accept: "application/pdf, application/octet-stream",
            },
            timeout: { request: requestTimeoutMs },
            retry: { limit: 0 },
            maxRedirects: 3,
            signal: controller.signal,
            hooks: {
              beforeRedirect: [
                (options) =>
                  void assertHttpsDocumentUrl(options.url, "Document redirects"),
              ],
            },
          });
          request.on("downloadProgress", ({ transferred }) => {
            if (transferred > MAX_PDF_BYTES) controller.abort();
          });
          const response = await request;
          return {
            body: response.rawBody,
            contentType: response.headers["content-type"],
          };
        } finally {
          signal.removeEventListener("abort", interrupt);
        }
      },
      catch: (cause) => failure("download PDF", cause),
    });
}
export function createPdfDownloader(
  requestTimeoutMs: number,
  requestPublicUrl: typeof publicGot = publicGot,
) {
  const effect = createPdfDownloaderEffect(requestTimeoutMs, requestPublicUrl);
  return (url: string): Promise<DownloadedFile> => runPromise(effect(url));
}
function assertHttpsDocumentUrl(
  value: string | URL | undefined,
  subject = "Document URL",
): URL {
  if (!value) throw new Error(`${subject} must be a public HTTPS URL`);
  const url = value instanceof URL ? value : new URL(value);
  if (url.protocol !== "https:" || url.username || url.password) {
    throw new Error(`${subject} must be a public HTTPS URL without credentials`);
  }
  return url;
}

function childProcess(
  executable: string,
  args: string[],
  binary: boolean,
  maxBuffer: number,
  environment?: Record<string, string>,
): Effect.Effect<string | Buffer, PrinterError> {
  return Effect.callback<string | Buffer, PrinterError>((resume) => {
    const child = execFileCallback(
      executable,
      args,
      {
        encoding: binary ? null : "utf8",
        env: { ...process.env, ...environment },
        maxBuffer,
        timeout: 60_000,
      },
      (error, stdout, stderr) => {
        if (!error) return resume(Effect.succeed(stdout));
        const detail = stderr.toString().trim();
        resume(
          Effect.fail(
            failure(
              `run ${executable}`,
              error,
              detail ? `${executable} failed: ${detail}` : `${executable} failed`,
            ),
          ),
        );
      },
    );
    return Effect.sync(() => child.kill("SIGTERM"));
  });
}
function validatePdf(download: DownloadedFile): Buffer {
  const type = download.contentType?.split(";", 1)[0]?.trim().toLowerCase();
  if (type !== "application/pdf" && type !== "application/octet-stream") {
    throw new Error("The document URL must return application/pdf");
  }
  const body = Buffer.from(download.body);
  if (body.length === 0 || body.length > MAX_PDF_BYTES) {
    throw new Error(`PDF must be between 1 byte and ${MAX_PDF_BYTES} bytes`);
  }
  if (body.subarray(0, 5).toString("ascii") !== "%PDF-") {
    throw new Error("The downloaded file is not a PDF");
  }
  return body;
}
function parsePdfInfo(stdout: string): number {
  if (/^Encrypted:\s*yes/im.test(stdout)) {
    throw new Error("Encrypted PDFs cannot be printed");
  }
  const rawPages = /^Pages:\s*(\d+)\s*$/im.exec(stdout)?.[1];
  const pages = rawPages ? Number.parseInt(rawPages, 10) : Number.NaN;
  if (!Number.isInteger(pages) || pages < 1) {
    throw new Error("Could not determine the PDF page count");
  }
  if (pages > MAX_PAGES) {
    throw new Error(`PDF has ${pages} pages; the maximum is ${MAX_PAGES}`);
  }
  return pages;
}
const firstRawValue = (value: unknown): unknown =>
  Array.isArray(value) ? value[0] : value;
function rawNumber(value: unknown): number | null {
  const first = firstRawValue(value);
  return typeof first === "number" && Number.isFinite(first) ? first : null;
}
function rawNonnegativeNumber(value: unknown): number | null {
  const number = rawNumber(value);
  return number !== null && number >= 0 ? number : null;
}
function rawPercent(value: unknown): number | null {
  const number = rawNumber(value);
  return number !== null && number >= 0 && number <= 100 ? number : null;
}
function rawStrings(value: unknown): string[] {
  return (Array.isArray(value) ? value : [value]).filter(
    (item): item is string => typeof item === "string",
  );
}
function jobState(value: unknown): PrintJob["state"] {
  const states: Record<number, PrintJob["state"]> = {
    3: "pending",
    4: "pending-held",
    5: "processing",
    6: "processing-stopped",
    7: "canceled",
    8: "aborted",
    9: "completed",
  };
  const state = rawNumber(value);
  return state === null ? "unknown" : (states[state] ?? `unknown:${state}`);
}
function createIppPrinterClient(url: string, timeout: number): PrinterClient {
  const printer = new Printer(url, { timeout });
  return {
    status: () => printer.status(),
    print: (data, options) => printer.print(data, options),
    jobStatus: async (jobUri) => {
      const response = await printer.client.request(IppOperation.GetJobAttributes, [
        {
          tag: IppTag.OperationAttributes,
          attributes: [
            { tag: IppTag.Charset, name: "attributes-charset", value: "utf-8" },
            {
              tag: IppTag.NaturalLanguage,
              name: "attributes-natural-language",
              value: "en",
            },
            { tag: IppTag.Uri, name: "job-uri", value: jobUri },
            {
              tag: IppTag.Keyword,
              name: "requested-attributes",
              value: ["job-state", "job-state-reasons", "job-impressions-completed"],
            },
          ],
        },
      ]);
      if (response.statusCode >= 0x0400) {
        throw new Error(
          `Could not read printer job status (IPP 0x${response.statusCode.toString(16)})`,
        );
      }
      const attributes = getAttributes(response, IppTag.JobAttributes);
      return {
        state: jobState(attributes["job-state"]),
        stateReasons: rawStrings(attributes["job-state-reasons"]),
        impressionsCompleted: rawNonnegativeNumber(
          attributes["job-impressions-completed"],
        ),
      };
    },
  };
}
function normalizeInput(input: PrintPdfInput) {
  return Schema.decodeUnknownEffect(PrintInputSchema)(input).pipe(
    Effect.mapError((cause) =>
      failure("validate print request", cause, "Invalid print request"),
    ),
    Effect.flatMap((decoded) => {
      const copies = decoded.copies ?? 1;
      if (!Number.isInteger(copies) || copies < 1 || copies > 3) {
        return Effect.fail(
          failure(
            "validate print request",
            input,
            "Copies must be an integer from 1 to 3",
          ),
        );
      }
      const jobName = decoded.jobName?.trim() || "MCP print job";
      if (jobName.length > 80 || /[\u0000-\u001f\u007f]/u.test(jobName)) {
        return Effect.fail(
          failure(
            "validate print request",
            input,
            "Job name must be at most 80 characters and contain no control characters",
          ),
        );
      }
      return Effect.succeed<NormalizedInput>({
        paper: decoded.paper ?? "letter",
        sides: decoded.sides ?? "two-sided-long-edge",
        copies,
        jobName,
      });
    }),
  );
}

export class IppPrinterService implements PrinterService {
  private readonly printer: PrinterClient | undefined;
  private readonly download: (
    url: string,
  ) => Effect.Effect<DownloadedFile, PrinterError>;
  private readonly runProcess: (
    executable: string,
    args: string[],
  ) => Effect.Effect<{ stdout: string; stderr: string }, PrinterError>;
  private readonly runBinaryProcess: (
    executable: string,
    args: string[],
    environment?: Record<string, string>,
  ) => Effect.Effect<Buffer, PrinterError>;
  private readonly now: () => number;
  private readonly tempRoot: string;
  private readonly maxPrintDataBytes: number;
  private readonly acceptedPrintStore: AcceptedPrintStore;
  private readonly volatileAcceptedPrints = new Map<string, number>();
  private readonly inFlightDuplicates = new Set<string>();

  constructor(options: PrinterServiceOptions = {}) {
    const dependencies = options.dependencies ?? {};
    this.printer =
      dependencies.printer ??
      (options.printerUrl
        ? createIppPrinterClient(
            options.printerUrl,
            options.printerTimeoutMs ?? 120_000,
          )
        : undefined);
    this.download = dependencies.download
      ? (url) => promiseCall("download PDF", () => dependencies.download!(url))
      : createPdfDownloaderEffect(options.requestTimeoutMs ?? 20_000);
    this.runProcess = dependencies.execFile
      ? (executable, args) =>
          promiseCall(`run ${executable}`, () =>
            dependencies.execFile!(executable, args),
          )
      : (executable, args) =>
          childProcess(executable, args, false, 1024 * 1024).pipe(
            Effect.map((stdout) => ({ stdout: stdout as string, stderr: "" })),
          );
    this.runBinaryProcess = dependencies.execFileBuffer
      ? (executable, args, environment) =>
          promiseCall(`run ${executable}`, () =>
            dependencies.execFileBuffer!(executable, args, environment),
          )
      : (executable, args, environment) =>
          childProcess(executable, args, true, MAX_PRINT_DATA_BYTES, environment).pipe(
            Effect.map((stdout) => stdout as Buffer),
          );
    this.now = dependencies.now ?? Date.now;
    this.tempRoot = dependencies.tempRoot ?? tmpdir();
    this.maxPrintDataBytes = dependencies.maxPrintDataBytes ?? MAX_PRINT_DATA_BYTES;
    this.acceptedPrintStore =
      dependencies.acceptedPrintStore ?? durableAcceptedPrintStore;
  }

  statusEffect(): Effect.Effect<PrinterStatus, PrinterError> {
    return Effect.gen({ self: this }, function* () {
      const printer = yield* this.requirePrinterEffect();
      const status = yield* promiseCall("read printer status", () => printer.status());
      const rawAccepting = firstRawValue(status.raw?.["printer-is-accepting-jobs"]);
      const acceptingJobs = typeof rawAccepting === "boolean" ? rawAccepting : null;
      return {
        configured: true,
        name: status.name,
        uri: status.uri,
        state: status.state,
        stateReasons: status.stateReasons,
        ready:
          acceptingJobs !== false &&
          (status.state === "idle" || status.state === "processing"),
        acceptingJobs,
        queuedJobCount: rawNonnegativeNumber(status.raw?.["queued-job-count"]),
        tonerPercent: rawPercent(status.raw?.["marker-levels"]),
        monochromeOnly: true,
        defaultSides: "two-sided-long-edge",
        supportedFormats: status.supportedFormats,
        supportedMedia: status.supportedMedia,
      };
    });
  }
  status(): Promise<PrinterStatus> {
    return runPromise(this.statusEffect());
  }

  printPdfEffect(input: PrintPdfInput): Effect.Effect<AcceptedPrintJob, PrinterError> {
    return Effect.gen({ self: this }, function* () {
      const printer = yield* this.requirePrinterEffect();
      const options = yield* normalizeInput(input);
      const url = yield* Effect.try({
        try: () => assertHttpsDocumentUrl(input.url),
        catch: (cause) => failure("validate document URL", cause),
      });
      const pdf = yield* this.download(url.href).pipe(
        Effect.flatMap((download) =>
          Effect.try({
            try: () => validatePdf(download),
            catch: (cause) => failure("validate PDF", cause),
          }),
        ),
      );
      const duplicateKey = createHash("sha256")
        .update(pdf)
        .update(JSON.stringify(options))
        .digest("hex");
      this.pruneDuplicates();
      if (
        !input.allowDuplicate &&
        (this.acceptedPrintStore.get(duplicateKey) ||
          this.volatileAcceptedPrints.has(duplicateKey))
      ) {
        return yield* Effect.fail(
          failure(
            "suppress duplicate",
            duplicateKey,
            "This exact document and print configuration was accepted within the last 5 minutes; set allowDuplicate to print it again",
          ),
        );
      }
      if (!input.allowDuplicate && this.inFlightDuplicates.has(duplicateKey)) {
        return yield* Effect.fail(
          failure(
            "reserve print job",
            duplicateKey,
            "This exact document and print configuration is already being submitted; wait for it to finish or set allowDuplicate to print another copy",
          ),
        );
      }
      const reserved = !input.allowDuplicate;
      if (reserved) this.inFlightDuplicates.add(duplicateKey);
      return yield* Effect.acquireUseRelease(
        promiseCall("create print workspace", () =>
          mkdtemp(join(this.tempRoot, "omni-printer-")),
        ),
        (directory) =>
          this.submitPrintJob(printer, directory, pdf, options, duplicateKey),
        (directory) =>
          promiseCall("remove print workspace", () =>
            rm(directory, { recursive: true, force: true }),
          ).pipe(
            // Cleanup must not replace the printer's authoritative accepted
            // result with a local filesystem failure.
            Effect.catch(() => Effect.void),
          ),
      ).pipe(
        Effect.ensuring(
          Effect.sync(() => {
            if (reserved) this.inFlightDuplicates.delete(duplicateKey);
          }),
        ),
      );
    });
  }
  printPdf(input: PrintPdfInput): Promise<AcceptedPrintJob> {
    return runPromise(this.printPdfEffect(input));
  }

  private submitPrintJob(
    printer: PrinterClient,
    directory: string,
    pdf: Buffer,
    options: NormalizedInput,
    duplicateKey: string,
  ): Effect.Effect<AcceptedPrintJob, PrinterError> {
    return Effect.gen({ self: this }, function* () {
      const inputPath = join(directory, "input.pdf");
      const rasterPath = join(directory, "output.raster");
      yield* this.writePrivateFile(inputPath, pdf);
      const info = yield* this.runProcess("pdfinfo", [inputPath]);
      const pages = yield* Effect.try({
        try: () => parsePdfInfo(info.stdout),
        catch: (cause) => failure("inspect PDF", cause),
      });
      const cupsOptions = [
        `PageSize=${cupsPaperNames[options.paper]}`,
        `Duplex=${cupsDuplexNames[options.sides]}`,
        "print-scaling=fit",
      ];
      const raster = yield* this.runBinaryProcess(CUPS_FILTER, [
        "-p",
        BRLASER_PPD,
        "-m",
        "application/vnd.cups-raster",
        "-i",
        "application/pdf",
        ...cupsOptions.flatMap((option) => ["-o", option]),
        inputPath,
      ]);
      yield* this.writePrivateFile(rasterPath, raster);
      const printData = yield* this.runBinaryProcess(
        BRLASER_FILTER,
        ["1", "omni", options.jobName, "1", cupsOptions.join(" "), rasterPath],
        { PPD: BRLASER_PPD },
      );
      if (printData.length === 0 || printData.length > this.maxPrintDataBytes) {
        return yield* Effect.fail(
          failure(
            "convert PDF",
            printData.length,
            `Converted print data exceeds the ${this.maxPrintDataBytes}-byte limit`,
          ),
        );
      }
      // Never retry submission: a transport failure can happen after acceptance.
      const job = yield* promiseCall("submit IPP job", () =>
        printer.print(printData, {
          copies: options.copies,
          media: mediaNames[options.paper],
          sides: options.sides,
          colorMode: "monochrome",
          documentFormat: "application/octet-stream",
          jobName: options.jobName,
          fitToPage: true,
        }),
      );
      // Acceptance is irreversible. Persist suppression before fallible polling
      // so a process restart cannot turn an uncertain retry into a second print.
      this.volatileAcceptedPrints.set(duplicateKey, this.now());
      const suppressionPersisted = yield* Effect.try({
        try: () => {
          this.acceptedPrintStore.upsert({
            fingerprint: duplicateKey,
            acceptedAt: this.now(),
            jobId: job.id,
            jobUri: job.uri,
            jobState: job.state,
            jobName: job.name,
          });
          return true as const;
        },
        catch: (cause) => failure("persist accepted print job", cause),
      }).pipe(Effect.catch(() => Effect.succeed(false as const)));
      const finalStatus = yield* this.waitForFinalJobStatus(printer, job);
      return {
        accepted: true,
        completed: finalStatus?.state === "completed",
        jobId: job.id,
        jobUri: job.uri,
        jobState: finalStatus?.state ?? job.state,
        jobName: job.name,
        pages,
        copies: options.copies,
        paper: options.paper,
        sides: options.sides,
        impressionsCompleted: finalStatus?.impressionsCompleted ?? null,
        message: !suppressionPersisted
          ? "The printer accepted the job, but durable duplicate suppression failed; do not retry it automatically"
          : finalStatus?.state === "completed"
            ? "The printer completed the job successfully"
            : "The printer accepted the job; physical completion is not confirmed",
      };
    });
  }
  private writePrivateFile(path: string, data: Uint8Array) {
    return promiseCall("write print workspace file", () =>
      writeFile(path, data, { mode: 0o600 }),
    );
  }
  private requirePrinterEffect(): Effect.Effect<PrinterClient, PrinterError> {
    return this.printer
      ? Effect.succeed(this.printer)
      : Effect.fail(
          failure("configure printer", undefined, "Printer is not configured"),
        );
  }
  private waitForFinalJobStatus(
    printer: PrinterClient,
    job: PrintJob,
  ): Effect.Effect<PrinterJobStatus | undefined, PrinterError> {
    if (!printer.jobStatus || job.id === null) return Effect.succeed(undefined);
    let latest: PrinterJobStatus | undefined;
    const poll = promiseCall("read printer job status", () =>
      printer.jobStatus!(job.uri),
    ).pipe(
      Effect.tap((status) =>
        Effect.sync(() => {
          latest = status;
        }),
      ),
      Effect.catchTag("PrinterError", () => Effect.succeed(undefined)),
    );
    return Effect.repeat(poll, {
      schedule: Schedule.addDelay(Schedule.recurs(JOB_STATUS_MAX_POLLS - 1), () =>
        Effect.succeed(Duration.millis(JOB_STATUS_POLL_MS)),
      ),
      until: (status) =>
        status === undefined ||
        status.state === "completed" ||
        status.state === "aborted" ||
        status.state === "canceled",
    }).pipe(
      Effect.asVoid,
      Effect.map(() => latest),
      Effect.flatMap((status) => {
        if (!status) return Effect.succeed(undefined);
        if (status.state === "aborted" || status.state === "canceled") {
          const reasons = status.stateReasons.join(", ") || "no reason reported";
          return Effect.fail(
            failure(
              "complete printer job",
              status,
              `Printer ${status.state} the job: ${reasons}`,
            ),
          );
        }
        return Effect.succeed(status.state === "completed" ? status : undefined);
      }),
    );
  }
  private pruneDuplicates(): void {
    const cutoff = this.now() - DUPLICATE_WINDOW_MS;
    this.acceptedPrintStore.deleteOlderThan(cutoff);
    for (const [fingerprint, acceptedAt] of this.volatileAcceptedPrints) {
      if (acceptedAt <= cutoff) this.volatileAcceptedPrints.delete(fingerprint);
    }
  }
}

export function createPrinterService(
  printerUrl: string,
  options: Omit<PrinterServiceOptions, "printerUrl"> = {},
): PrinterService {
  return new IppPrinterService({ ...options, printerUrl });
}
