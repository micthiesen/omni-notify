//! Taste evidence and profile storage.

use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};

use super::types::{TasteEvidenceData, TasteProfileData};

/// Inserts new evidence without ever mutating an observation already
/// recorded; returns how many rows were new.
pub async fn insert_taste_evidence(
    store: &Store,
    evidence: Vec<TasteEvidenceData>,
) -> Result<u64, StoreError> {
    store
        .write(move |tx| {
            let mut inserted = 0u64;
            for item in &evidence {
                if tx.has::<TasteEvidenceData>(&item.evidence_id)? {
                    continue;
                }
                tx.upsert(item, UpsertOpts::default())?;
                inserted += 1;
            }
            Ok(inserted)
        })
        .await
}

/// Newest observation first.
pub async fn get_all_taste_evidence(store: &Store) -> Result<Vec<TasteEvidenceData>, StoreError> {
    let mut all = store
        .read(|docs| docs.get_all::<TasteEvidenceData>())
        .await?;
    all.sort_by_key(|e| std::cmp::Reverse(e.observed_at));
    Ok(all)
}

/// Profile ids are immutable checkpoints; `false` when the id already exists.
pub async fn insert_taste_profile(
    store: &Store,
    profile: TasteProfileData,
) -> Result<bool, StoreError> {
    store
        .write(move |tx| {
            if tx.has::<TasteProfileData>(&profile.profile.profile_id)? {
                return Ok(false);
            }
            tx.upsert(&profile, UpsertOpts::default())?;
            Ok(true)
        })
        .await
}

/// The highest version (newest generation breaks ties).
pub fn latest_profile(mut profiles: Vec<TasteProfileData>) -> Option<TasteProfileData> {
    profiles.sort_by(|a, b| {
        b.profile
            .version
            .cmp(&a.profile.version)
            .then(b.profile.generated_at.cmp(&a.profile.generated_at))
    });
    profiles.into_iter().next()
}

pub async fn get_latest_taste_profile(
    store: &Store,
) -> Result<Option<TasteProfileData>, StoreError> {
    let all = store
        .read(|docs| docs.get_all::<TasteProfileData>())
        .await?;
    Ok(latest_profile(all))
}
