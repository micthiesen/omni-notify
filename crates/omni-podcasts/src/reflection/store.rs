//! Evidence ledger and profile checkpoints.

use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};

use super::types::{PodcastTasteEvidenceData, PodcastTasteProfileData};

/// Append-only: existing rows are never rewritten. Returns the number inserted.
pub async fn insert_podcast_taste_evidence(
    store: &Store,
    evidence: Vec<PodcastTasteEvidenceData>,
) -> Result<usize, StoreError> {
    store
        .write(move |tx| {
            let mut inserted = 0;
            for item in &evidence {
                if tx.has::<PodcastTasteEvidenceData>(&item.evidence_id)? {
                    continue;
                }
                tx.upsert(item, UpsertOpts::default())?;
                inserted += 1;
            }
            Ok::<_, StoreError>(inserted)
        })
        .await
}

/// All evidence, newest observation first.
pub async fn get_all_podcast_taste_evidence(
    store: &Store,
) -> Result<Vec<PodcastTasteEvidenceData>, StoreError> {
    let mut evidence = store
        .read(|docs| docs.get_all::<PodcastTasteEvidenceData>())
        .await?;
    evidence.sort_by_key(|e| std::cmp::Reverse(e.observed_at));
    Ok(evidence)
}

/// Immutable checkpoint: `false` (no write) when the id already exists.
pub async fn insert_podcast_taste_profile(
    store: &Store,
    profile: PodcastTasteProfileData,
) -> Result<bool, StoreError> {
    store
        .write(move |tx| {
            if tx.has::<PodcastTasteProfileData>(&profile.profile_id)? {
                return Ok::<_, StoreError>(false);
            }
            tx.upsert(&profile, UpsertOpts::default())?;
            Ok(true)
        })
        .await
}

/// Highest version, then newest generation.
pub fn latest_profile(
    mut profiles: Vec<PodcastTasteProfileData>,
) -> Option<PodcastTasteProfileData> {
    profiles.sort_by(|a, b| {
        b.version
            .cmp(&a.version)
            .then(b.generated_at.cmp(&a.generated_at))
    });
    profiles.into_iter().next()
}

pub async fn get_latest_podcast_taste_profile(
    store: &Store,
) -> Result<Option<PodcastTasteProfileData>, StoreError> {
    Ok(latest_profile(
        store
            .read(|docs| docs.get_all::<PodcastTasteProfileData>())
            .await?,
    ))
}
