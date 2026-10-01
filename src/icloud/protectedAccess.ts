import { Data, Effect, Schema } from "effect";

// PCS exchange adapted from MIT-licensed pyicloud, commit
// b5f2e2a7f9e5cd5be7e009626c4ae021d8b2bb34. See docs/licenses/pyicloud-MIT.txt.
type Operation = "PCS state" | "PCS consent" | "PCS cookies";
type Endpoint = "requestWebAccessState" | "enableDeviceConsentForPCS" | "requestPCS";

export class ICloudPcsError extends Data.TaggedError("ICloudPcsError")<{
  readonly operation: Operation;
  readonly status?: number;
  readonly reason:
    | "invalid-app-name"
    | "http"
    | "invalid-response"
    | "consent-not-sent"
    | "unknown-state";
}> {}

const StateSchema = Schema.Struct({
  isICDRSDisabled: Schema.Boolean,
  isDeviceConsentedForPCS: Schema.optional(Schema.Boolean),
});
const ConsentSchema = Schema.Struct({
  isDeviceConsentNotificationSent: Schema.Boolean,
});
const CookiesSchema = Schema.Struct({
  status: Schema.String,
  message: Schema.optional(Schema.String),
});

/** The caller owns authenticated requests, cookie persistence, and account locking. */
export const requestProtectedAccess = Effect.fn("requestProtectedAccess")(function* <E>(
  request: (
    operation: Operation,
    endpoint: Endpoint,
    body?: unknown,
  ) => Effect.Effect<{ status: number; data: unknown }, E>,
  appName: string,
): Effect.fn.Return<"not-required" | "consent-required" | "ready", E | ICloudPcsError> {
  if (appName.length > 64 || !/^[a-z][a-z0-9-]*$/.test(appName)) {
    return yield* Effect.fail(
      new ICloudPcsError({ operation: "PCS state", reason: "invalid-app-name" }),
    );
  }

  const decodedRequest = <A, I>(
    operation: Operation,
    endpoint: Endpoint,
    schema: Schema.Codec<A, I>,
    body?: unknown,
  ): Effect.Effect<A, E | ICloudPcsError> =>
    Effect.gen(function* () {
      const response = yield* request(operation, endpoint, body);
      if (response.status !== 200) {
        return yield* Effect.fail(
          new ICloudPcsError({ operation, status: response.status, reason: "http" }),
        );
      }
      return yield* Schema.decodeUnknownEffect(schema)(response.data).pipe(
        Effect.mapError(
          () =>
            new ICloudPcsError({ operation, status: 200, reason: "invalid-response" }),
        ),
      );
    });

  const state = yield* decodedRequest(
    "PCS state",
    "requestWebAccessState",
    StateSchema,
  );
  if (state.isICDRSDisabled !== true) return "not-required";

  if (state.isDeviceConsentedForPCS !== true) {
    const consent = yield* decodedRequest(
      "PCS consent",
      "enableDeviceConsentForPCS",
      ConsentSchema,
    );
    if (!consent.isDeviceConsentNotificationSent) {
      return yield* Effect.fail(
        new ICloudPcsError({
          operation: "PCS consent",
          status: 200,
          reason: "consent-not-sent",
        }),
      );
    }
    return "consent-required";
  }

  for (let attempt = 0; attempt < 10; attempt++) {
    const response = yield* decodedRequest("PCS cookies", "requestPCS", CookiesSchema, {
      appName,
      derivedFromUserAction: attempt === 0,
    });
    if (response.status === "success") return "ready";
    if (
      response.message !== "Requested the device to upload cookies." &&
      response.message !== "Cookies not available yet on server."
    ) {
      return yield* Effect.fail(
        new ICloudPcsError({
          operation: "PCS cookies",
          status: 200,
          reason: "unknown-state",
        }),
      );
    }
    if (attempt < 9) yield* Effect.sleep("5 seconds");
  }
  return "consent-required";
});
