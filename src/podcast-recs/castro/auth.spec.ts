import { describe, expect, it } from "vitest";
import {
  createCastroAuthHeaders,
  createCastroCanonicalString,
  hashCastroRequestBody,
  signCastroCanonicalString,
} from "./auth.js";

describe("Castro API authentication", () => {
  it("hashes an empty request body like the captured Castro client", () => {
    expect(hashCastroRequestBody()).toBe(
      "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
    );
  });

  it("builds the APIAuth canonical string", () => {
    expect(
      createCastroCanonicalString(
        {
          method: "post",
          pathAndQuery: "/ctrl_api/v1/json",
          date: "Thu, 25 Aug 2022 04:27:52 GMT",
        },
        "OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=",
      ),
    ).toBe(
      "POST,application/json,OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=,/ctrl_api/v1/json,Thu, 25 Aug 2022 04:27:52 GMT",
    );
  });

  it("signs the public APIAuth-HMAC-SHA256 test vector", () => {
    const canonical =
      "POST,application/json,OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=,/ctrl_api/v1/json,Thu, 25 Aug 2022 04:27:52 GMT";
    const secret = Buffer.from(
      "AGnO/VenzHB9xkLYZG1i70kQ9iyFBBvugGXSFyTQaB0=",
      "base64",
    );

    expect(signCastroCanonicalString(canonical, secret)).toBe(
      "vPI9MMRwBZLWNrCcnLnbJjZRna0+XP7yFMhc9KMUFdw=",
    );
  });

  it("returns all headers required by Castro", () => {
    const headers = createCastroAuthHeaders(
      { accessId: "device-id", secret: Buffer.from("secret") },
      {
        method: "GET",
        pathAndQuery: "/ping",
        date: "Thu, 16 Jul 2026 17:26:59 GMT",
      },
    );

    expect(headers.Authorization).toMatch(
      /^APIAuth-HMAC-SHA256 device-id:[A-Za-z0-9+/]{43}=$/,
    );
    expect(headers).toMatchObject({
      "Content-Type": "application/json",
      Date: "Thu, 16 Jul 2026 17:26:59 GMT",
      "X-Authorization-Content-SHA256": "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
    });
  });
});
