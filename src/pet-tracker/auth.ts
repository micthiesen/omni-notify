import { Logger } from "@micthiesen/mitools/logging";
import {
  AuthenticationDetails,
  CognitoUser,
  CognitoUserPool,
} from "amazon-cognito-identity-js";

const USER_POOL_ID = "us-east-1_rjhNnZVAm";
const CLIENT_ID = "4552ujeu3aic90nf8qn53levmn";

const logger = new Logger("WhiskerAuth");

const TOKEN_EXPIRY_BUFFER_MS = 5 * 60 * 1000; // Re-auth 5 min before expiry

const userPool = new CognitoUserPool({
  UserPoolId: USER_POOL_ID,
  ClientId: CLIENT_ID,
});

let cachedAuth: { idToken: string; userId: string; expiresAt: number } | null = null;

export async function authenticateWhisker(
  email: string,
  password: string,
): Promise<{ idToken: string; userId: string }> {
  if (cachedAuth && Date.now() < cachedAuth.expiresAt - TOKEN_EXPIRY_BUFFER_MS) {
    logger.debug("Using cached credentials");
    return { idToken: cachedAuth.idToken, userId: cachedAuth.userId };
  }

  const authDetails = new AuthenticationDetails({
    Username: email,
    Password: password,
  });

  const cognitoUser = new CognitoUser({
    Username: email,
    Pool: userPool,
  });

  const session = await new Promise<CognitoUserSession>((resolve, reject) => {
    cognitoUser.authenticateUser(authDetails, {
      onSuccess: (result) => resolve(result),
      onFailure: (err) => reject(err),
    });
  });

  const idToken = session.getIdToken().getJwtToken();
  const payload = decodeJwtPayload(idToken);
  const userId = payload.mid;

  if (typeof userId !== "string") {
    throw new Error("Missing 'mid' claim in id token");
  }

  const exp = payload.exp;
  const expiresAt = typeof exp === "number" ? exp * 1000 : Date.now() + 60 * 60 * 1000;

  cachedAuth = { idToken, userId, expiresAt };
  logger.debug("Authenticated (fresh)", { userId });
  return { idToken, userId };
}

type CognitoUserSession = ReturnType<CognitoUser["getSignInUserSession"]> &
  NonNullable<unknown>;

function decodeJwtPayload(token: string): Record<string, unknown> {
  const parts = token.split(".");
  if (parts.length !== 3) {
    throw new Error("Invalid JWT format");
  }
  const payload = Buffer.from(parts[1], "base64url").toString("utf-8");
  return JSON.parse(payload);
}
