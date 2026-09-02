import { mkdtemp, readdir, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { publicGot } from "../press-pods/publicHttp.js";
import {
  type AcceptedPrintRecord,
  createPdfDownloaderEffect,
  IppPrinterService,
  type PrinterServiceDependencies,
} from "./service.js";

const PDF = Buffer.from("%PDF-1.7\nfixture");
const statusOf = (service: IppPrinterService) =>
  Effect.runPromise(service.statusEffect());
const printPdf = (
  service: IppPrinterService,
  input: Parameters<IppPrinterService["printPdfEffect"]>[0],
) => Effect.runPromise(service.printPdfEffect(input));

function printerStatus(raw: Record<string, unknown> = {}) {
  return {
    name: "Brother HL-L2370DW",
    uri: "ipp://10.10.1.47:631/ipp/print",
    state: "idle" as const,
    stateReasons: [],
    supportedFormats: ["application/octet-stream"],
    supportedMedia: ["na_letter_8.5x11in"],
    readyMedia: null,
    supportedResolutions: [],
    resolution: null,
    raw,
  };
}

describe("IppPrinterService", () => {
  let tempRoot: string;
  let print: ReturnType<typeof vi.fn>;
  let status: ReturnType<typeof vi.fn>;
  let jobStatus: ReturnType<typeof vi.fn>;
  let processCalls: Array<{ executable: string; args: string[] }>;
  let binaryProcessCalls: Array<{
    executable: string;
    args: string[];
    environment?: Record<string, string>;
  }>;
  let dependencies: PrinterServiceDependencies;

  beforeEach(async () => {
    tempRoot = await mkdtemp(join(tmpdir(), "omni-printer-test-"));
    processCalls = [];
    binaryProcessCalls = [];
    print = vi.fn().mockResolvedValue({
      id: 42,
      uri: "ipp://10.10.1.47/jobs/42",
      state: "pending",
      name: "MCP print job",
    });
    status = vi.fn().mockResolvedValue(printerStatus());
    jobStatus = vi.fn().mockResolvedValue({
      state: "completed",
      stateReasons: ["job-completed-successfully"],
      impressionsCompleted: 2,
    });
    const acceptedPrints = new Map<string, AcceptedPrintRecord>();
    dependencies = {
      tempRoot,
      now: () => 1_000_000,
      download: vi.fn().mockResolvedValue({
        body: PDF,
        contentType: "application/pdf; charset=binary",
      }),
      wait: vi.fn().mockResolvedValue(undefined),
      printer: { print, status, jobStatus } as NonNullable<
        PrinterServiceDependencies["printer"]
      >,
      acceptedPrintStore: {
        get: (fingerprint) => acceptedPrints.get(fingerprint),
        upsert: (record) => acceptedPrints.set(record.fingerprint, record),
        deleteOlderThan: (cutoff) => {
          for (const [fingerprint, record] of acceptedPrints) {
            if (record.acceptedAt <= cutoff) acceptedPrints.delete(fingerprint);
          }
        },
      },
      execFile: async (executable, args) => {
        processCalls.push({ executable, args });
        if (executable === "pdfinfo") {
          const inputStat = await stat(args[0]);
          expect(inputStat.mode & 0o777).toBe(0o600);
          return { stdout: "Pages: 2\nEncrypted: no\n", stderr: "" };
        }
        throw new Error(`Unexpected process: ${executable}`);
      },
      execFileBuffer: async (executable, args, environment) => {
        binaryProcessCalls.push({ executable, args, environment });
        return executable.endsWith("cupsfilter")
          ? Buffer.from("cups-raster-fixture")
          : Buffer.from("brlaser-fixture");
      },
    };
  });

  afterEach(async () => {
    await rm(tempRoot, { recursive: true, force: true });
  });

  it("normalizes printer readiness and consumable status", async () => {
    status.mockResolvedValue(
      printerStatus({
        "printer-is-accepting-jobs": false,
        "queued-job-count": 3,
        "marker-levels": [61, 100],
      }),
    );
    const service = new IppPrinterService({ dependencies });

    await expect(statusOf(service)).resolves.toMatchObject({
      configured: true,
      state: "idle",
      ready: false,
      acceptingJobs: false,
      queuedJobCount: 3,
      tonerPercent: 61,
      monochromeOnly: true,
      defaultSides: "two-sided-long-edge",
    });
  });

  it("converts, submits, and confirms a monochrome duplex brlaser job", async () => {
    const service = new IppPrinterService({ dependencies });
    const result = await printPdf(service, {
      url: "https://example.com/file.pdf",
      paper: "a4",
      copies: 2,
      jobName: "Board packet",
    });

    expect(result).toMatchObject({
      accepted: true,
      completed: true,
      jobId: 42,
      pages: 2,
      copies: 2,
      paper: "a4",
      sides: "two-sided-long-edge",
      impressionsCompleted: 2,
    });
    expect(result.message).toContain("completed the job successfully");
    expect(processCalls[0]).toMatchObject({ executable: "pdfinfo" });
    expect(binaryProcessCalls[0]?.executable).toBe("/usr/sbin/cupsfilter");
    expect(binaryProcessCalls[0]?.args).toEqual(
      expect.arrayContaining([
        "PageSize=A4",
        "Duplex=DuplexNoTumble",
        "print-scaling=fit",
      ]),
    );
    expect(binaryProcessCalls[1]).toMatchObject({
      executable: "/usr/lib/cups/filter/rastertobrlaser",
      environment: {
        PPD: "/usr/share/omni-printing/brother-hll2370dw.ppd",
      },
    });
    expect(binaryProcessCalls[1]?.args).toEqual(
      expect.arrayContaining(["PageSize=A4 Duplex=DuplexNoTumble print-scaling=fit"]),
    );
    expect(print).toHaveBeenCalledWith(
      expect.any(Buffer),
      expect.objectContaining({
        copies: 2,
        media: "iso_a4_210x297mm",
        sides: "two-sided-long-edge",
        colorMode: "monochrome",
        documentFormat: "application/octet-stream",
        jobName: "Board packet",
      }),
    );
    expect(print.mock.calls[0]?.[0]).toEqual(Buffer.from("brlaser-fixture"));
    expect(jobStatus).toHaveBeenCalledWith("ipp://10.10.1.47/jobs/42");
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it.each([
    ["one-sided", "Duplex=None"],
    ["two-sided-short-edge", "Duplex=DuplexTumble"],
  ] as const)("passes %s through both CUPS filters", async (sides, duplexOption) => {
    const service = new IppPrinterService({ dependencies });

    await printPdf(service, { url: "https://example.com/file.pdf", sides });

    expect(binaryProcessCalls[0]?.args).toEqual(expect.arrayContaining([duplexOption]));
    expect(binaryProcessCalls[1]?.args).toEqual(
      expect.arrayContaining([`PageSize=Letter ${duplexOption} print-scaling=fit`]),
    );
  });

  it("reports a printer-aborted job as a failure", async () => {
    jobStatus.mockResolvedValue({
      state: "aborted",
      stateReasons: ["document-format-error"],
      impressionsCompleted: 0,
    });
    const service = new IppPrinterService({ dependencies });

    await expect(
      printPdf(service, { url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("Printer aborted the job: document-format-error");
  });

  it.each([
    ["text/html", PDF, "must return application/pdf"],
    ["application/pdf", Buffer.from("not a pdf"), "not a PDF"],
    [
      "application/pdf",
      Buffer.concat([Buffer.from("%PDF-"), Buffer.alloc(20 * 1024 * 1024)]),
      "PDF must be between",
    ],
  ])("rejects an invalid download", async (contentType, body, message) => {
    dependencies.download = vi.fn().mockResolvedValue({ contentType, body });
    const service = new IppPrinterService({ dependencies });

    await expect(
      printPdf(service, { url: "https://example.com/file.pdf" }),
    ).rejects.toThrow(message);
    expect(print).not.toHaveBeenCalled();
    expect(processCalls).toEqual([]);
  });

  it("rejects encrypted and oversized-page PDFs and cleans temporary files", async () => {
    dependencies.execFile = vi.fn().mockResolvedValue({
      stdout: "Pages: 26\nEncrypted: yes\n",
      stderr: "",
    });
    const service = new IppPrinterService({ dependencies });

    await expect(
      printPdf(service, { url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("Encrypted PDFs cannot be printed");
    expect(print).not.toHaveBeenCalled();
    expect(await readdir(tempRoot)).toEqual([]);

    dependencies.execFile = vi.fn().mockResolvedValue({
      stdout: "Pages: 26\nEncrypted: no\n",
      stderr: "",
    });
    await expect(
      printPdf(new IppPrinterService({ dependencies }), {
        url: "https://example.com/file.pdf",
      }),
    ).rejects.toThrow("maximum is 25");
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("rejects oversized converted print data before submission", async () => {
    dependencies.maxPrintDataBytes = 10;
    dependencies.execFileBuffer = vi.fn().mockResolvedValue(Buffer.alloc(11));
    const service = new IppPrinterService({ dependencies });

    await expect(
      printPdf(service, { url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("Converted print data exceeds");
    expect(print).not.toHaveBeenCalled();
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("suppresses only accepted exact duplicates for five minutes", async () => {
    let currentTime = 10_000;
    dependencies.now = () => currentTime;
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await printPdf(service, input);
    await expect(printPdf(service, input)).rejects.toThrow("allowDuplicate");
    await printPdf(service, { ...input, allowDuplicate: true });
    currentTime += 5 * 60 * 1000 + 1;
    await printPdf(service, input);
    expect(print).toHaveBeenCalledTimes(3);
  });

  it("suppresses an accepted duplicate after the service restarts", async () => {
    const input = { url: "https://example.com/file.pdf" };
    await printPdf(new IppPrinterService({ dependencies }), input);

    await expect(
      printPdf(new IppPrinterService({ dependencies }), input),
    ).rejects.toThrow("allowDuplicate");
    expect(print).toHaveBeenCalledTimes(1);
  });

  it("reports acceptance without inviting a retry when durable suppression fails", async () => {
    dependencies.acceptedPrintStore = {
      get: () => undefined,
      upsert: () => {
        throw new Error("database unavailable");
      },
      deleteOlderThan: () => undefined,
    };
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await expect(printPdf(service, input)).resolves.toMatchObject({
      accepted: true,
      message:
        "The printer accepted the job, but durable duplicate suppression failed; do not retry it automatically",
    });
    await expect(printPdf(service, input)).rejects.toThrow("allowDuplicate");
    expect(print).toHaveBeenCalledTimes(1);
  });

  it("prevents concurrent duplicate submissions unless explicitly allowed", async () => {
    let releasePrint: (() => void) | undefined;
    print.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          releasePrint = () =>
            resolve({
              id: 42,
              uri: "ipp://10.10.1.47/jobs/42",
              state: "pending",
              name: "MCP print job",
            });
        }),
    );
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    const first = printPdf(service, input);
    await vi.waitFor(() => expect(print).toHaveBeenCalledTimes(1));
    await expect(printPdf(service, input)).rejects.toThrow("already being submitted");
    releasePrint?.();
    await expect(first).resolves.toMatchObject({ accepted: true });
    expect(print).toHaveBeenCalledTimes(1);
  });

  it("does not suppress a retry when IPP submission fails", async () => {
    print.mockRejectedValueOnce(new Error("printer connection closed"));
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await expect(printPdf(service, input)).rejects.toThrow("connection closed");
    await expect(printPdf(service, input)).resolves.toMatchObject({ accepted: true });
    expect(print).toHaveBeenCalledTimes(2);
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("suppresses duplicates immediately after IPP acceptance when status polling fails", async () => {
    jobStatus.mockRejectedValue(new Error("Get-Job-Attributes timed out"));
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await expect(printPdf(service, input)).resolves.toMatchObject({
      accepted: true,
      completed: false,
      message: "The printer accepted the job; physical completion is not confirmed",
    });
    await expect(printPdf(service, input)).rejects.toThrow("allowDuplicate");
    expect(print).toHaveBeenCalledTimes(1);
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("validates the public input shape before downloading", async () => {
    const service = new IppPrinterService({ dependencies });

    await expect(
      printPdf(service, { url: "http://example.com/file.pdf" }),
    ).rejects.toThrow("public HTTPS URL");
    await expect(
      printPdf(service, { url: "https://example.com/file.pdf", copies: 4 }),
    ).rejects.toThrow("Copies must be an integer from 1 to 3");
    expect(dependencies.download).not.toHaveBeenCalled();
  });

  it("rejects an HTTPS redirect that downgrades to HTTP", async () => {
    const request = vi.fn((_url: string | URL, options = {}) => {
      const hook = options.hooks?.beforeRedirect?.[0];
      hook?.({ url: new URL("http://example.com/file.pdf") } as never, {} as never);
      throw new Error("expected redirect hook to reject HTTP");
    }) as unknown as typeof publicGot;
    const download = createPdfDownloaderEffect(1_000, request);

    await expect(
      Effect.runPromise(download("https://example.com/file.pdf")),
    ).rejects.toThrow(
      "Document redirects must be a public HTTPS URL without credentials",
    );
  });

  it("writes the downloaded bytes unchanged before inspection", async () => {
    dependencies.execFile = async (executable, args) => {
      if (executable === "pdfinfo") {
        expect(await readFile(args[0])).toEqual(PDF);
        throw new Error("inspection stopped");
      }
      throw new Error("unexpected process");
    };
    const service = new IppPrinterService({ dependencies });
    await expect(
      printPdf(service, { url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("inspection stopped");
    expect(await readdir(tempRoot)).toEqual([]);
  });
});
