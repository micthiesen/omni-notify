import { Logger } from "@micthiesen/mitools/logging";
import {
  AuthenticationDetails,
  CognitoUser,
  CognitoUserPool,
} from "amazon-cognito-identity-js";
import { Clock, Data, Effect, Ref, Schema } from "effect";

const USER_POOL_ID = "us-east-1_rjhNnZVAm";
const CLIENT_ID = "4552ujeu3aic90nf8qn53levmn";

const logger = new Logger("WhiskerAuth");

const TOKEN_EXPIRY_BUFFER_MS = 5 * 60 * 1000; // Re-auth 5 min before expiry

const userPool = new CognitoUserPool({
  UserPoolId: USER_POOL_ID,
  ClientId: CLIENT_ID,
});

const cachedAuth = Ref.makeUnsafe<{
  idToken: string;
  userId: string;
  expiresAt: number;
} | null>(null);

export class WhiskerAuthenticationError extends Data.TaggedError(
  "WhiskerAuthenticationError",
)<{ readonly cause: unknown }> {
  public override get message(): string {
    return `Whisker authentication failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

const JwtPayload = Schema.Struct({
  mid: Schema.String,
  exp: Schema.optional(Schema.Number),
});

export function authenticateWhisker(
  email: string,
  password: string,
): Effect.Effect<{ idToken: string; userId: string }, WhiskerAuthenticationError> {
  return Effect.gen(function* () {
    const cached = yield* Ref.get(cachedAuth);
    const now = yield* Clock.currentTimeMillis;
    if (cached && now < cached.expiresAt - TOKEN_EXPIRY_BUFFER_MS) {
      logger.debug("Using cached credentials");
      return { idToken: cached.idToken, userId: cached.userId };
    }

    const authDetails = new AuthenticationDetails({
      Username: email,
      Password: password,
    });

    const cognitoUser = new CognitoUser({
      Username: email,
      Pool: userPool,
    });

    const session = yield* Effect.callback<
      CognitoUserSession,
      WhiskerAuthenticationError
    >((resume) => {
      cognitoUser.authenticateUser(authDetails, {
        onSuccess: (result) => resume(Effect.succeed(result)),
        onFailure: (cause) =>
          resume(Effect.fail(new WhiskerAuthenticationError({ cause }))),
      });
    });

    const idToken = session.getIdToken().getJwtToken();
    const payload = yield* decodeJwtPayload(idToken);
    const userId = payload.mid;
    const expiresAt = payload.exp ? payload.exp * 1000 : now + 60 * 60 * 1000;

    yield* Ref.set(cachedAuth, { idToken, userId, expiresAt });
    logger.debug("Authenticated (fresh)", { userId });
    return { idToken, userId };
  });
}

type CognitoUserSession = ReturnType<CognitoUser["getSignInUserSession"]> &
  NonNullable<unknown>;

function decodeJwtPayload(
  token: string,
): Effect.Effect<Schema.Schema.Type<typeof JwtPayload>, WhiskerAuthenticationError> {
  return Effect.gen(function* () {
    const parts = token.split(".");
    if (parts.length !== 3 || !parts[1]) {
      return yield* new WhiskerAuthenticationError({
        cause: new Error("Invalid JWT format"),
      });
    }
    const unknownPayload = yield* Effect.try({
      try: () => JSON.parse(Buffer.from(parts[1], "base64url").toString("utf-8")),
      catch: (cause) => new WhiskerAuthenticationError({ cause }),
    });
    return yield* Schema.decodeUnknownEffect(JwtPayload)(unknownPayload).pipe(
      Effect.mapError((cause) => new WhiskerAuthenticationError({ cause })),
    );
  });
}
