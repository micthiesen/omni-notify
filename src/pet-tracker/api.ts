import { Logger } from "@micthiesen/mitools/logging";
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
  weightHistory: WeightReading[];
}

interface GraphQLResponse<T> {
  data?: T;
  errors?: { message: string }[];
}

async function graphqlRequest<T>(
  idToken: string,
  query: string,
  variables: Record<string, unknown>,
): Promise<T> {
  logger.debug(`GraphQL request: ${query.match(/query (\w+)/)?.[1] ?? "unknown"}`);

  const response = await got
    .post(GRAPHQL_ENDPOINT, {
      headers: {
        Authorization: `Bearer ${idToken}`,
        "Content-Type": "application/json",
      },
      json: { query, variables },
      timeout: { request: 15_000 },
    })
    .json<GraphQLResponse<T>>();

  if (response.errors?.length) {
    const messages = response.errors.map((e) => e.message).join("; ");
    throw new Error(`GraphQL errors: ${messages}`);
  }

  if (!response.data) {
    throw new Error("GraphQL response missing data");
  }

  return response.data;
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

export async function fetchPetsByUser(
  idToken: string,
  userId: string,
): Promise<WhiskerPet[]> {
  const data = await graphqlRequest<{ getPetsByUser: WhiskerPet[] }>(
    idToken,
    GET_PETS_BY_USER,
    { userId },
  );
  return data.getPetsByUser;
}

export async function fetchWeightHistory(
  idToken: string,
  petId: string,
  limit?: number,
): Promise<WeightReading[]> {
  const variables: Record<string, unknown> = { petId };
  if (limit !== undefined) {
    variables.limit = limit;
  }

  const data = await graphqlRequest<{
    getWeightHistoryByPetId: WeightReading[];
  }>(idToken, GET_WEIGHT_HISTORY_BY_PET_ID, variables);
  return data.getWeightHistoryByPetId;
}
