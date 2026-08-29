import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import {
  type PrinterStatus as IppPrinterStatus,
  Printer,
  type PrintJob,
  type PrintJobOptions,
} from "@pnosolutions/ipp";
import { publicGot } from "../press-pods/publicHttp.js";

const execFileAsync = promisify(execFileCallback);

export const PRINTER_DOWNLOAD_USER_AGENT =
  "OpenAI File Downloader, XaiImageApiFetch/1.0";

const MAX_PDF_BYTES = 20 * 1024 * 1024;
const MAX_RASTER_BYTES = 256 * 1024 * 1024;
const MAX_PAGES = 25;
const DUPLICATE_WINDOW_MS = 5 * 60 * 1000;

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
  jobId: number | null;
  jobUri: string;
  jobState: PrintJob["state"];
  jobName: string;
  pages: number;
  copies: number;
  paper: PrintPaper;
  sides: PrintSides;
  message: string;
}

export interface PrinterService {
  status(): Promise<PrinterStatus>;
  printPdf(input: PrintPdfInput): Promise<AcceptedPrintJob>;
}

interface DownloadedFile {
  body: Uint8Array;
  contentType: string | undefined;
}

interface PrinterClient {
  status(): Promise<IppPrinterStatus & { raw?: Record<string, unknown> }>;
  print(data: Uint8Array, options?: PrintJobOptions): Promise<PrintJob>;
}

type ProcessRunner = (
  executable: string,
  args: string[],
) => Promise<{ stdout: string; stderr: string }>;

export interface PrinterServiceDependencies {
  download?: (url: string) => Promise<DownloadedFile>;
  execFile?: ProcessRunner;
  printer?: PrinterClient;
  now?: () => number;
  tempRoot?: string;
}

export interface PrinterServiceOptions {
  printerUrl?: string;
  requestTimeoutMs?: number;
  dependencies?: PrinterServiceDependencies;
}

const mediaNames: Record<PrintPaper, string> = {
  letter: "na_letter_8.5x11in",
  a4: "iso_a4_210x297mm",
  legal: "na_legal_8.5x14in",
};

export function createPdfDownloader(
  requestTimeoutMs: number,
  requestPublicUrl: typeof publicGot = publicGot,
) {
  return async (url: string): Promise<DownloadedFile> => {
    const sizeController = new AbortController();
    const request = requestPublicUrl(url, {
      headers: {
        "User-Agent": PRINTER_DOWNLOAD_USER_AGENT,
        Accept: "application/pdf, application/octet-stream",
      },
      timeout: { request: requestTimeoutMs },
      retry: { limit: 0 },
      maxRedirects: 3,
      signal: sizeController.signal,
      hooks: {
        beforeRedirect: [
          (options) => {
            assertHttpsDocumentUrl(options.url, "Document redirects");
          },
        ],
      },
    });
    request.on("downloadProgress", ({ transferred }) => {
      if (transferred > MAX_PDF_BYTES) sizeController.abort();
    });
    const response = await request;
    return {
      body: response.rawBody,
      contentType: response.headers["content-type"],
    };
  };
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

const defaultProcessRunner: ProcessRunner = async (executable, args) => {
  const result = await execFileAsync(executable, args, {
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
    timeout: 60_000,
  });
  return { stdout: result.stdout, stderr: result.stderr };
};

function validatePdf(download: DownloadedFile): Buffer {
  const contentType = download.contentType?.split(";", 1)[0]?.trim().toLowerCase();
  if (contentType !== "application/pdf" && contentType !== "application/octet-stream") {
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
  const encrypted = /^Encrypted:\s*(\S+)/im.exec(stdout)?.[1]?.toLowerCase();
  if (encrypted === "yes") throw new Error("Encrypted PDFs cannot be printed");
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

function firstRawValue(value: unknown): unknown {
  return Array.isArray(value) ? value[0] : value;
}

function rawBoolean(value: unknown): boolean | null {
  const first = firstRawValue(value);
  return typeof first === "boolean" ? first : null;
}

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

function normalizedInput(input: PrintPdfInput) {
  const paper = input.paper ?? "letter";
  const sides = input.sides ?? "two-sided-long-edge";
  const copies = input.copies ?? 1;
  if (!(["letter", "a4", "legal"] as const).includes(paper)) {
    throw new Error("Paper must be letter, a4, or legal");
  }
  if (
    !(["one-sided", "two-sided-long-edge", "two-sided-short-edge"] as const).includes(
      sides,
    )
  ) {
    throw new Error(
      "Sides must be one-sided, two-sided-long-edge, or two-sided-short-edge",
    );
  }
  if (!Number.isInteger(copies) || copies < 1 || copies > 3) {
    throw new Error("Copies must be an integer from 1 to 3");
  }
  const jobName = input.jobName?.trim() || "MCP print job";
  if (jobName.length > 80 || /[\u0000-\u001f\u007f]/u.test(jobName)) {
    throw new Error(
      "Job name must be at most 80 characters and contain no control characters",
    );
  }
  return { paper, sides, copies, jobName };
}

export class IppPrinterService implements PrinterService {
  private readonly printer: PrinterClient | undefined;
  private readonly download: (url: string) => Promise<DownloadedFile>;
  private readonly runProcess: ProcessRunner;
  private readonly now: () => number;
  private readonly tempRoot: string;
  private readonly acceptedDuplicates = new Map<string, number>();
  private readonly inFlightDuplicates = new Set<string>();

  constructor(options: PrinterServiceOptions = {}) {
    const dependencies = options.dependencies ?? {};
    this.printer =
      dependencies.printer ??
      (options.printerUrl
        ? new Printer(options.printerUrl, {
            timeout: options.requestTimeoutMs ?? 10_000,
          })
        : undefined);
    this.download =
      dependencies.download ?? createPdfDownloader(options.requestTimeoutMs ?? 20_000);
    this.runProcess = dependencies.execFile ?? defaultProcessRunner;
    this.now = dependencies.now ?? Date.now;
    this.tempRoot = dependencies.tempRoot ?? tmpdir();
  }

  async status(): Promise<PrinterStatus> {
    const printer = this.requirePrinter();
    const status = await printer.status();
    const acceptingJobs = rawBoolean(status.raw?.["printer-is-accepting-jobs"]);
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
  }

  async printPdf(input: PrintPdfInput): Promise<AcceptedPrintJob> {
    const printer = this.requirePrinter();
    const url = assertHttpsDocumentUrl(input.url);
    const options = normalizedInput(input);
    const pdf = validatePdf(await this.download(url.href));
    const duplicateKey = createHash("sha256")
      .update(pdf)
      .update(JSON.stringify(options))
      .digest("hex");
    this.pruneDuplicates();
    if (!input.allowDuplicate && this.acceptedDuplicates.has(duplicateKey)) {
      throw new Error(
        "This exact document and print configuration was accepted within the last 5 minutes; set allowDuplicate to print it again",
      );
    }
    if (!input.allowDuplicate && this.inFlightDuplicates.has(duplicateKey)) {
      throw new Error(
        "This exact document and print configuration is already being submitted; wait for it to finish or set allowDuplicate to print another copy",
      );
    }
    const reserved = !input.allowDuplicate;
    if (reserved) this.inFlightDuplicates.add(duplicateKey);

    let directory: string | undefined;
    try {
      directory = await mkdtemp(join(this.tempRoot, "omni-printer-"));
      const inputPath = join(directory, "input.pdf");
      const outputPath = join(directory, "output.pwg");
      await writeFile(inputPath, pdf, { mode: 0o600 });
      const info = await this.runProcess("pdfinfo", [inputPath]);
      const pages = parsePdfInfo(info.stdout);
      await this.runProcess("gs", [
        "-dSAFER",
        "-dBATCH",
        "-dNOPAUSE",
        "-sDEVICE=pwgraster",
        "-r600",
        "-sColorConversionStrategy=Gray",
        "-dProcessColorModel=/DeviceGray",
        "-dFIXEDMEDIA",
        "-dPDFFitPage",
        `-sPAPERSIZE=${options.paper}`,
        `-sOutputFile=${outputPath}`,
        inputPath,
      ]);
      const rasterStat = await stat(outputPath);
      if (rasterStat.size === 0 || rasterStat.size > MAX_RASTER_BYTES) {
        throw new Error(
          `Converted print data exceeds the ${MAX_RASTER_BYTES}-byte limit`,
        );
      }
      const raster = await readFile(outputPath);

      // Do not retry this call: a transport failure may happen after the printer
      // accepted the job, and resubmission could print a second copy.
      const job = await printer.print(raster, {
        copies: options.copies,
        media: mediaNames[options.paper],
        sides: options.sides,
        colorMode: "monochrome",
        documentFormat: "image/pwg-raster",
        jobName: options.jobName,
        fitToPage: true,
      });
      this.acceptedDuplicates.set(duplicateKey, this.now());
      return {
        accepted: true,
        jobId: job.id,
        jobUri: job.uri,
        jobState: job.state,
        jobName: job.name,
        pages,
        copies: options.copies,
        paper: options.paper,
        sides: options.sides,
        message: "The printer accepted the job; physical completion is not confirmed",
      };
    } finally {
      if (directory) await rm(directory, { recursive: true, force: true });
      if (reserved) this.inFlightDuplicates.delete(duplicateKey);
    }
  }

  private requirePrinter(): PrinterClient {
    if (!this.printer) throw new Error("Printer is not configured");
    return this.printer;
  }

  private pruneDuplicates(): void {
    const cutoff = this.now() - DUPLICATE_WINDOW_MS;
    for (const [key, acceptedAt] of this.acceptedDuplicates) {
      if (acceptedAt <= cutoff) this.acceptedDuplicates.delete(key);
    }
  }
}

export function createPrinterService(
  printerUrl: string,
  options: Omit<PrinterServiceOptions, "printerUrl"> = {},
): PrinterService {
  return new IppPrinterService({ ...options, printerUrl });
}
