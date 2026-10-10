# TODO

Ideas that are agreed-on but deliberately not built yet.

- **Nightly `docstore.db` backup task.** The SQLite docstore is the entire state of
  the app (streamer state, recommendations, feedback, task runs).
  A scheduled task should snapshot it nightly (SQLite online backup API or
  `VACUUM INTO`) and rotate a handful of copies, ideally to a destination outside
  the container volume.
- **Tests for the untested podcast-recs modules.** `selection.rs`, `shortlist.rs`,
  `discovery.rs`, and `guests.rs` in `crates/omni-podcasts/src/` have no tests.
  Extract their pure parts (prompt assembly, candidate mapping, result validation)
  and test those, mirroring how `crates/omni-media/tests/shortlist.rs` covers the
  media-recs equivalents.
