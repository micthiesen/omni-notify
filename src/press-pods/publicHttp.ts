import { promises as dns, lookup } from "node:dns";
import { BlockList, isIP, type LookupFunction } from "node:net";
import got, { type OptionsInit } from "got";

const blockedAddresses = new BlockList();

for (const [network, prefix] of [
  ["0.0.0.0", 8],
  ["10.0.0.0", 8],
  ["100.64.0.0", 10],
  ["127.0.0.0", 8],
  ["169.254.0.0", 16],
  ["172.16.0.0", 12],
  ["192.0.0.0", 24],
  ["192.0.2.0", 24],
  ["192.168.0.0", 16],
  ["198.18.0.0", 15],
  ["198.51.100.0", 24],
  ["203.0.113.0", 24],
  ["224.0.0.0", 4],
  ["240.0.0.0", 4],
] as const) {
  blockedAddresses.addSubnet(network, prefix, "ipv4");
}

for (const [network, prefix] of [
  ["::", 128],
  ["::1", 128],
  ["64:ff9b::", 96],
  ["64:ff9b:1::", 48],
  ["100::", 64],
  ["2001::", 32],
  ["2001:2::", 48],
  ["2001:10::", 28],
  ["2001:db8::", 32],
  ["2002::", 16],
  ["fc00::", 7],
  ["fe80::", 10],
  ["ff00::", 8],
] as const) {
  blockedAddresses.addSubnet(network, prefix, "ipv6");
}

function normalizedMappedIpv4(address: string): string | undefined {
  const match = /^::ffff:(\d{1,3}(?:\.\d{1,3}){3})$/i.exec(address);
  if (match) return match[1];

  const embedded = /^::(?:ffff:)?([\da-f]{1,4}):([\da-f]{1,4})$/i.exec(address);
  if (!embedded) return undefined;
  const value =
    Number.parseInt(embedded[1], 16) * 0x1_0000 + Number.parseInt(embedded[2], 16);
  return [value >>> 24, (value >>> 16) & 255, (value >>> 8) & 255, value & 255].join(
    ".",
  );
}

export function isPublicAddress(address: string): boolean {
  const mapped = normalizedMappedIpv4(address);
  if (mapped) return isPublicAddress(mapped);

  const family = isIP(address);
  if (family === 4) return !blockedAddresses.check(address, "ipv4");
  if (family === 6) return !blockedAddresses.check(address, "ipv6");
  return false;
}

export function assertPublicHttpUrlSyntax(value: string | URL): URL {
  const url = value instanceof URL ? value : new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("PressPods URLs must use HTTP or HTTPS");
  }
  if (url.username || url.password) {
    throw new Error("PressPods URLs must not contain credentials");
  }
  const hostname = url.hostname
    .toLowerCase()
    .replace(/\.$/, "")
    .replace(/^\[|\]$/g, "");
  if (!hostname || hostname === "localhost" || hostname.endsWith(".localhost")) {
    throw new Error("PressPods URLs must use a public host");
  }
  if (isIP(hostname) && !isPublicAddress(hostname)) {
    throw new Error("PressPods URLs must use a public host");
  }
  return url;
}

export async function assertPublicHttpUrl(value: string | URL): Promise<URL> {
  const url = assertPublicHttpUrlSyntax(value);
  const hostname = url.hostname.replace(/^\[|\]$/g, "");
  if (isIP(hostname)) return url;

  const addresses = await dns.lookup(hostname, { all: true, verbatim: true });
  if (
    addresses.length === 0 ||
    addresses.some(({ address }) => !isPublicAddress(address))
  ) {
    throw new Error("PressPods URLs must resolve only to public addresses");
  }
  return url;
}

/**
 * Resolve every outbound connection through a public-address gate. Got invokes
 * this for each redirect, so a public URL cannot redirect into the LAN or a
 * loopback/link-local endpoint. IP-literal requests are rejected by the hook.
 */
export function createPublicDnsLookup(
  resolve: LookupFunction = lookup,
): LookupFunction {
  return (hostname, options, callback) => {
    resolve(
      hostname,
      { ...options, verbatim: true },
      (error, address, resultFamily) => {
        const resolvedAddresses =
          typeof address === "string"
            ? [{ address, family: resultFamily ?? isIP(address) }]
            : (address ?? []);
        if (error) {
          callback(error, options.all ? [] : "", resultFamily);
          return;
        }
        if (
          resolvedAddresses.length === 0 ||
          resolvedAddresses.some(
            ({ address: resolvedAddress }) => !isPublicAddress(resolvedAddress),
          )
        ) {
          callback(
            new Error("PressPods URLs must resolve only to public addresses"),
            options.all ? [] : "",
            resultFamily,
          );
          return;
        }
        if (options.all) {
          callback(null, resolvedAddresses);
          return;
        }
        const first = resolvedAddresses[0];
        callback(null, first.address, first.family);
      },
    );
  };
}

const publicDnsLookup = createPublicDnsLookup();

export function publicGot(url: string | URL, options: OptionsInit = {}) {
  const existingBeforeRequest = options.hooks?.beforeRequest ?? [];
  const existingBeforeRedirect = options.hooks?.beforeRedirect ?? [];
  return got(url, {
    ...options,
    dnsLookup: publicDnsLookup,
    hooks: {
      ...options.hooks,
      beforeRequest: [
        async (requestOptions) => {
          if (requestOptions.url) await assertPublicHttpUrl(requestOptions.url);
        },
        ...existingBeforeRequest,
      ],
      beforeRedirect: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...existingBeforeRedirect,
      ],
    },
  });
}

export async function fetchPublicHtml(url: string, userAgent: string): Promise<string> {
  return publicGot(url, {
    headers: { "User-Agent": userAgent, Accept: "text/html" },
    timeout: { request: 20_000 },
    retry: { limit: 2, methods: ["GET"] },
  }).text();
}
