//! Pure IMAP folder-sync planning (`src/email/imap/sync.ts`). New-mail
//! detection is UID based: everything at or above the cursor's uidNext is new.
//! CONDSTORE/QRESYNC are deliberately unused (iCloud rejects parameterized
//! `SELECT ... (CONDSTORE)`).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderState {
    pub uid_validity: String,
    pub uid_next: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderSyncPlan {
    /// First contact with this folder: record the cursor, skip history.
    Init,
    /// Nothing new since the cursor.
    None,
    /// Fetch UIDs >= `from_uid`.
    Fetch { from_uid: u32 },
    /// UIDVALIDITY changed: UIDs are meaningless, recover by received date.
    Reset,
}

pub fn plan_folder_sync(cursor: Option<&FolderState>, status: &FolderState) -> FolderSyncPlan {
    let Some(cursor) = cursor else {
        return FolderSyncPlan::Init;
    };
    if cursor.uid_validity != status.uid_validity {
        return FolderSyncPlan::Reset;
    }
    if status.uid_next > cursor.uid_next {
        return FolderSyncPlan::Fetch {
            from_uid: cursor.uid_next,
        };
    }
    FolderSyncPlan::None
}

#[cfg(test)]
mod sync_spec {
    use super::*;

    fn state(uid_validity: &str, uid_next: u32) -> FolderState {
        FolderState {
            uid_validity: uid_validity.to_owned(),
            uid_next,
        }
    }

    #[test]
    fn initializes_on_first_contact_with_a_folder() {
        assert_eq!(
            plan_folder_sync(None, &state("1000", 50)),
            FolderSyncPlan::Init
        );
    }

    #[test]
    fn does_nothing_when_uid_next_is_unchanged() {
        assert_eq!(
            plan_folder_sync(Some(&state("1000", 50)), &state("1000", 50)),
            FolderSyncPlan::None
        );
    }

    #[test]
    fn fetches_from_the_cursor_when_new_uids_exist() {
        assert_eq!(
            plan_folder_sync(Some(&state("1000", 50)), &state("1000", 53)),
            FolderSyncPlan::Fetch { from_uid: 50 }
        );
    }

    #[test]
    fn resets_when_uidvalidity_changes() {
        assert_eq!(
            plan_folder_sync(Some(&state("1000", 50)), &state("2000", 3)),
            FolderSyncPlan::Reset
        );
    }

    #[test]
    fn treats_a_lower_uid_next_with_same_validity_as_nothing_new() {
        assert_eq!(
            plan_folder_sync(Some(&state("1000", 50)), &state("1000", 40)),
            FolderSyncPlan::None
        );
    }
}
