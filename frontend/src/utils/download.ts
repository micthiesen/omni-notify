/** Trigger a client-side file download of in-memory content. */
export function downloadFile(
  filename: string,
  content: string,
  mimeType: string,
): void {
  const url = URL.createObjectURL(new Blob([content], { type: mimeType }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  // Revoke on a delay: revoking synchronously can race the download start.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
