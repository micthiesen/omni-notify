//! The dashboard "On Deck" strip.

use futures::future::BoxFuture;
use omni_api::media::OnDeckItem;
use omni_runtime::ports::{OnDeckSource, PortError};
use omni_store::{Store, StoreError};
use serde_json::Value;

use crate::persistence::{get_open_recommendations, select_on_deck};
use crate::routes::serialize_on_deck;

/// The newest delivered recommendations still awaiting an outcome.
pub async fn build_on_deck(store: &Store) -> Result<Vec<OnDeckItem>, StoreError> {
    let open = get_open_recommendations(store).await?;
    Ok(select_on_deck(&open)
        .iter()
        .map(serialize_on_deck)
        .collect())
}

/// [`OnDeckSource`] over the recommendation store (set by app wiring).
#[derive(Clone)]
pub struct MediaOnDeck {
    store: Store,
}

impl MediaOnDeck {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl OnDeckSource for MediaOnDeck {
    fn on_deck(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async move {
            let items = build_on_deck(&self.store)
                .await
                .map_err(|e| PortError::Failed {
                    message: e.to_string(),
                    transient: false,
                })?;
            items
                .iter()
                .map(|item| {
                    serde_json::to_value(item).map_err(|e| PortError::Failed {
                        message: e.to_string(),
                        transient: false,
                    })
                })
                .collect()
        })
    }
}
