import {
  mkdtemp,
  readdir,
  readFile,
  rm,
  stat,
  truncate,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { publicGot } from "../press-pods/publicHttp.js";
import {
  createPdfDownloader,
  IppPrinterService,
  type PrinterServiceDependencies,
} from "./service.js";

const PDF = Buffer.from("%PDF-1.7\nfixture");

function printerStatus(raw: Record<string, unknown> = {}) {
  return {
    name: "Brother HL-L2370DW",
    uri: "ipp://10.10.1.47:631/ipp/print",
    state: "idle" as const,
    stateReasons: [],
    supportedFormats: ["image/pwg-raster"],
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
  let processCalls: Array<{ executable: string; args: string[] }>;
  let dependencies: PrinterServiceDependencies;

  beforeEach(async () => {
    tempRoot = await mkdtemp(join(tmpdir(), "omni-printer-test-"));
    processCalls = [];
    print = vi.fn().mockResolvedValue({
      id: 42,
      uri: "ipp://10.10.1.47/jobs/42",
      state: "pending",
      name: "MCP print job",
    });
    status = vi.fn().mockResolvedValue(printerStatus());
    dependencies = {
      tempRoot,
      now: () => 1_000_000,
      download: vi.fn().mockResolvedValue({
        body: PDF,
        contentType: "application/pdf; charset=binary",
      }),
      printer: { print, status } as NonNullable<PrinterServiceDependencies["printer"]>,
      execFile: async (executable, args) => {
        processCalls.push({ executable, args });
        if (executable === "pdfinfo") {
          const inputStat = await stat(args[0]);
          expect(inputStat.mode & 0o777).toBe(0o600);
          return { stdout: "Pages: 2\nEncrypted: no\n", stderr: "" };
        }
        const outputPath = args
          .find((arg) => arg.startsWith("-sOutputFile="))
          ?.slice("-sOutputFile=".length);
        if (!outputPath) throw new Error("Missing Ghostscript output path");
        await writeFile(outputPath, Buffer.from("RaS2fixture"));
        return { stdout: "", stderr: "" };
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

    await expect(service.status()).resolves.toMatchObject({
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

  it("converts and submits a monochrome duplex PWG raster job", async () => {
    const service = new IppPrinterService({ dependencies });
    const result = await service.printPdf({
      url: "https://example.com/file.pdf",
      paper: "a4",
      copies: 2,
      jobName: "Board packet",
    });

    expect(result).toMatchObject({
      accepted: true,
      jobId: 42,
      pages: 2,
      copies: 2,
      paper: "a4",
      sides: "two-sided-long-edge",
    });
    expect(result.message).toContain("physical completion is not confirmed");
    expect(processCalls[0]).toMatchObject({ executable: "pdfinfo" });
    expect(processCalls[1]?.args).toEqual(
      expect.arrayContaining([
        "-dSAFER",
        "-sDEVICE=pwgraster",
        "-r600",
        "-sColorConversionStrategy=Gray",
        "-dFIXEDMEDIA",
        "-dPDFFitPage",
        "-sPAPERSIZE=a4",
      ]),
    );
    expect(print).toHaveBeenCalledWith(
      expect.any(Buffer),
      expect.objectContaining({
        copies: 2,
        media: "iso_a4_210x297mm",
        sides: "two-sided-long-edge",
        colorMode: "monochrome",
        documentFormat: "image/pwg-raster",
        jobName: "Board packet",
      }),
    );
    expect(await readdir(tempRoot)).toEqual([]);
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
      service.printPdf({ url: "https://example.com/file.pdf" }),
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
      service.printPdf({ url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("Encrypted PDFs cannot be printed");
    expect(print).not.toHaveBeenCalled();
    expect(await readdir(tempRoot)).toEqual([]);

    dependencies.execFile = vi.fn().mockResolvedValue({
      stdout: "Pages: 26\nEncrypted: no\n",
      stderr: "",
    });
    await expect(
      new IppPrinterService({ dependencies }).printPdf({
        url: "https://example.com/file.pdf",
      }),
    ).rejects.toThrow("maximum is 25");
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("rejects an oversized raster before submission", async () => {
    dependencies.execFile = async (executable, args) => {
      if (executable === "pdfinfo") {
        return { stdout: "Pages: 1\nEncrypted: no\n", stderr: "" };
      }
      const outputPath = args
        .find((arg) => arg.startsWith("-sOutputFile="))
        ?.slice("-sOutputFile=".length);
      if (!outputPath) throw new Error("Missing Ghostscript output path");
      await writeFile(outputPath, "x");
      await truncate(outputPath, 256 * 1024 * 1024 + 1);
      return { stdout: "", stderr: "" };
    };
    const service = new IppPrinterService({ dependencies });

    await expect(
      service.printPdf({ url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("Converted print data exceeds");
    expect(print).not.toHaveBeenCalled();
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("suppresses only accepted exact duplicates for five minutes", async () => {
    let currentTime = 10_000;
    dependencies.now = () => currentTime;
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await service.printPdf(input);
    await expect(service.printPdf(input)).rejects.toThrow("allowDuplicate");
    await service.printPdf({ ...input, allowDuplicate: true });
    currentTime += 5 * 60 * 1000 + 1;
    await service.printPdf(input);
    expect(print).toHaveBeenCalledTimes(3);
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

    const first = service.printPdf(input);
    await vi.waitFor(() => expect(print).toHaveBeenCalledTimes(1));
    await expect(service.printPdf(input)).rejects.toThrow("already being submitted");
    releasePrint?.();
    await expect(first).resolves.toMatchObject({ accepted: true });
    expect(print).toHaveBeenCalledTimes(1);
  });

  it("does not suppress a retry when IPP submission fails", async () => {
    print.mockRejectedValueOnce(new Error("printer connection closed"));
    const service = new IppPrinterService({ dependencies });
    const input = { url: "https://example.com/file.pdf" };

    await expect(service.printPdf(input)).rejects.toThrow("connection closed");
    await expect(service.printPdf(input)).resolves.toMatchObject({ accepted: true });
    expect(print).toHaveBeenCalledTimes(2);
    expect(await readdir(tempRoot)).toEqual([]);
  });

  it("validates the public input shape before downloading", async () => {
    const service = new IppPrinterService({ dependencies });

    await expect(
      service.printPdf({ url: "http://example.com/file.pdf" }),
    ).rejects.toThrow("public HTTPS URL");
    await expect(
      service.printPdf({ url: "https://example.com/file.pdf", copies: 4 }),
    ).rejects.toThrow("Copies must be an integer from 1 to 3");
    expect(dependencies.download).not.toHaveBeenCalled();
  });

  it("rejects an HTTPS redirect that downgrades to HTTP", async () => {
    const request = vi.fn((_url: string | URL, options = {}) => {
      const hook = options.hooks?.beforeRedirect?.[0];
      hook?.({ url: new URL("http://example.com/file.pdf") } as never, {} as never);
      throw new Error("expected redirect hook to reject HTTP");
    }) as unknown as typeof publicGot;
    const download = createPdfDownloader(1_000, request);

    await expect(download("https://example.com/file.pdf")).rejects.toThrow(
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
      service.printPdf({ url: "https://example.com/file.pdf" }),
    ).rejects.toThrow("inspection stopped");
    expect(await readdir(tempRoot)).toEqual([]);
  });
});
