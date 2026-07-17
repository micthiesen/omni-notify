# TODO

Ideas that are agreed-on but deliberately not built yet.

- **Nightly `docstore.db` backup task.** The SQLite docstore is the entire state of
  the app (streamer state, recommendations, feedback, task runs, briefing history).
  A scheduled task should snapshot it nightly (SQLite online backup API or
  `VACUUM INTO`) and rotate a handful of copies, ideally to a destination outside
  the container volume.
- **Specs for the untested podcast-recs modules.** `selection.ts`, `shortlist.ts`,
  `discovery.ts`, and `guests.ts` have no `.spec.ts`. Extract their pure parts
  (prompt assembly, candidate mapping, result validation) and test those, mirroring
  how `src/recommendations/shortlist.spec.ts` covers the media-recs equivalents.
