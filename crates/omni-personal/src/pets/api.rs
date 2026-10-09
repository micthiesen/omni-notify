//! Whisker pet-profile GraphQL (`src/pet-tracker/api.ts`).

use std::time::Duration;

use omni_http::{HttpClient, Method, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub const GRAPHQL_ENDPOINT: &str = "https://pet-profile.iothings.site/graphql/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const LOG: &str = "pet-tracker:api";

/// One scale reading; `timestamp` is Whisker's text, stored verbatim.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WeightReading {
    /// Pounds.
    pub weight: f64,
    pub timestamp: String,
}

/// A pet profile with its last seven days of readings.
#[derive(Clone, Debug, PartialEq)]
pub struct WhiskerPet {
    pub pet_id: String,
    pub name: String,
    /// Pounds.
    pub weight: f64,
    pub last_weight_reading: f64,
    pub weight_history: Vec<WeightReading>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation} failed: {cause}")]
pub struct WhiskerApiError {
    pub operation: String,
    pub cause: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PetPayload {
    pet_id: String,
    name: String,
    weight: f64,
    last_weight_reading: f64,
    // Whisker returns null instead of [] when the last seven days have no readings.
    #[serde(deserialize_with = "crate::reset_alerts::source::required_nullable")]
    weight_history: Option<Vec<WeightReading>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PetsData {
    get_pets_by_user: Vec<PetPayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryData {
    get_weight_history_by_pet_id: Vec<WeightReading>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
struct GraphQlResponse<T> {
    #[serde(
        default,
        deserialize_with = "crate::reset_alerts::source::optional_non_null"
    )]
    data: Option<T>,
    #[serde(
        default,
        deserialize_with = "crate::reset_alerts::source::optional_non_null"
    )]
    errors: Option<Vec<GraphQlError>>,
}

const GET_PETS_BY_USER: &str = "
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
";

const GET_WEIGHT_HISTORY_BY_PET_ID: &str = "
  query GetWeightHistoryByPetId($petId: String!, $limit: Int) {
    getWeightHistoryByPetId(petId: $petId, limit: $limit) {
      weight
      timestamp
    }
  }
";

fn operation_name(query: &str) -> String {
    query
        .split("query ")
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Whisker GraphQL request".to_owned())
}

/// The Whisker GraphQL client.
#[derive(Clone)]
pub struct WhiskerApi {
    http: HttpClient,
}

impl WhiskerApi {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        id_token: &str,
        query: &str,
        variables: Value,
    ) -> Result<T, WhiskerApiError> {
        let operation = operation_name(query);
        let fail = |cause: String| WhiskerApiError {
            operation: operation.clone(),
            cause,
        };
        tracing::debug!(target: LOG, "GraphQL request: {operation}");
        let url = Url::parse(GRAPHQL_ENDPOINT).map_err(|e| fail(e.to_string()))?;
        let response = self
            .http
            .request(Method::POST, url)
            .bearer_auth(id_token)
            .json(&json!({"query": query, "variables": variables}))
            .timeout(REQUEST_TIMEOUT)
            .send_bounded(MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| fail(e.to_string()))?;
        if !response.status.is_success() {
            // got's `HTTPError` message.
            return Err(fail(format!(
                "Response code {} ({})",
                response.status.as_u16(),
                response.status.canonical_reason().unwrap_or("")
            )));
        }
        let decoded: GraphQlResponse<T> =
            serde_json::from_slice(&response.body).map_err(|e| fail(e.to_string()))?;
        if let Some(errors) = decoded.errors.filter(|errors| !errors.is_empty()) {
            let messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
            return Err(fail(format!("GraphQL errors: {}", messages.join("; "))));
        }
        decoded
            .data
            .ok_or_else(|| fail("GraphQL response missing data".to_owned()))
    }

    /// `fetchPetsByUser`; a null history becomes empty.
    pub async fn fetch_pets_by_user(
        &self,
        id_token: &str,
        user_id: &str,
    ) -> Result<Vec<WhiskerPet>, WhiskerApiError> {
        let data: PetsData = self
            .request(id_token, GET_PETS_BY_USER, json!({"userId": user_id}))
            .await?;
        Ok(data
            .get_pets_by_user
            .into_iter()
            .map(|pet| WhiskerPet {
                pet_id: pet.pet_id,
                name: pet.name,
                weight: pet.weight,
                last_weight_reading: pet.last_weight_reading,
                weight_history: pet.weight_history.unwrap_or_default(),
            })
            .collect())
    }

    /// `fetchWeightHistory`.
    pub async fn fetch_weight_history(
        &self,
        id_token: &str,
        pet_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<WeightReading>, WhiskerApiError> {
        let mut variables = Map::new();
        variables.insert("petId".to_owned(), Value::String(pet_id.to_owned()));
        if let Some(limit) = limit {
            variables.insert("limit".to_owned(), Value::from(limit));
        }
        let data: HistoryData = self
            .request(
                id_token,
                GET_WEIGHT_HISTORY_BY_PET_ID,
                Value::Object(variables),
            )
            .await?;
        Ok(data.get_weight_history_by_pet_id)
    }
}
