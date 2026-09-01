import { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect, Schema } from "effect";
import got from "got";

const GRAPHQL_ENDPOINT = "https://pet-profile.iothings.site/graphql/";

const logger = new Logger("pet-tracker:api");

export interface WeightReading {
  weight: number; // lbs
  timestamp: string; // ISO timestamp
}

export interface WhiskerPet {
  petId: string;
  name: string;
  weight: number; // lbs
  lastWeightReading: number;
  weightHistory: readonly WeightReading[];
}

export class WhiskerApiError extends Data.TaggedError("WhiskerApiError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation} failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

const WeightReadingSchema = Schema.Struct({
  weight: Schema.Number,
  timestamp: Schema.String,
});
const WhiskerPetSchema = Schema.Struct({
  petId: Schema.String,
  name: Schema.String,
  weight: Schema.Number,
  lastWeightReading: Schema.Number,
  weightHistory: Schema.Array(WeightReadingSchema),
});
const GraphQlErrorSchema = Schema.Struct({ message: Schema.String });

function graphqlRequest<A, I>(
  idToken: string,
  query: string,
  variables: Record<string, unknown>,
  dataSchema: Schema.Codec<A, I>,
): Effect.Effect<A, WhiskerApiError> {
  const operation = query.match(/query (\w+)/)?.[1] ?? "Whisker GraphQL request";
  const responseSchema = Schema.Struct({
    data: Schema.optional(dataSchema),
    errors: Schema.optional(Schema.Array(GraphQlErrorSchema)),
  });
  return Effect.gen(function* () {
    logger.debug(`GraphQL request: ${operation}`);
    const response = yield* Effect.tryPromise({
      try: (signal) =>
        got
          .post(GRAPHQL_ENDPOINT, {
            headers: {
              Authorization: `Bearer ${idToken}`,
              "Content-Type": "application/json",
            },
            json: { query, variables },
            timeout: { request: 15_000 },
            signal,
          })
          .json<unknown>(),
      catch: (cause) => new WhiskerApiError({ operation, cause }),
    });
    const decoded = yield* Schema.decodeUnknownEffect(responseSchema)(response).pipe(
      Effect.mapError((cause) => new WhiskerApiError({ operation, cause })),
    );
    if (decoded.errors?.length) {
      return yield* new WhiskerApiError({
        operation,
        cause: new Error(
          `GraphQL errors: ${decoded.errors.map((error) => error.message).join("; ")}`,
        ),
      });
    }
    if (!decoded.data) {
      return yield* new WhiskerApiError({
        operation,
        cause: new Error("GraphQL response missing data"),
      });
    }
    return decoded.data;
  });
}

const GET_PETS_BY_USER = `
  query GetPetsByUser($userId: String!) {
    getPetsByUser(userId: $userId) {
      petId
      name
      weight
      lastWeightReading
      weightHistory {
        weight
        timestamp
      }
    }
  }
`;

const GET_WEIGHT_HISTORY_BY_PET_ID = `
  query GetWeightHistoryByPetId($petId: String!, $limit: Int) {
    getWeightHistoryByPetId(petId: $petId, limit: $limit) {
      weight
      timestamp
    }
  }
`;

export function fetchPetsByUser(
  idToken: string,
  userId: string,
): Effect.Effect<readonly WhiskerPet[], WhiskerApiError> {
  return graphqlRequest(
    idToken,
    GET_PETS_BY_USER,
    { userId },
    Schema.Struct({ getPetsByUser: Schema.Array(WhiskerPetSchema) }),
  ).pipe(Effect.map((data) => data.getPetsByUser));
}

export function fetchWeightHistory(
  idToken: string,
  petId: string,
  limit?: number,
): Effect.Effect<readonly WeightReading[], WhiskerApiError> {
  const variables: Record<string, unknown> = { petId };
  if (limit !== undefined) {
    variables.limit = limit;
  }

  return graphqlRequest(
    idToken,
    GET_WEIGHT_HISTORY_BY_PET_ID,
    variables,
    Schema.Struct({
      getWeightHistoryByPetId: Schema.Array(WeightReadingSchema),
    }),
  ).pipe(Effect.map((data) => data.getWeightHistoryByPetId));
}
