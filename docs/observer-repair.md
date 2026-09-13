# Observer issue repair

`ObserverRepair` runs every 15 minutes and on startup. Observer is the Overseerr
service at media.syas.ca (`http://overseerr:5055` inside Compose).
Set `OBSERVER_URL` and `OBSERVER_API_KEY` to register the task. It uses the existing
Sonarr/Radarr, OpenAI, and recommendations Pushover credentials. Set
`OBSERVER_REPAIR_ENABLED=false` to disable it.

Luna (`openai:gpt-5.6-luna`) gets current Arr availability plus read tools for the
reported title and ten recent resolved issues. Its standard loop has at most
16 steps and returns validated JSON. Code executes the chosen action:

- Replace wrong or broken media: match the current file's import to its grabbed
  release, blocklist that release, verify it, delete and verify the scoped files,
  then submit an automatic search.
- Missing media: search only the missing monitored episodes/movie.
- Cannot handle: leave the issue open and notify Michael with the reason.

The report defines the scope ceiling. An exact quoted user comment can narrow a
series/season report to specific episodes. Title identity comes from TMDB/TVDB,
never names or model-provided Arr IDs. Shared files and season-pack releases that
extend outside the selected scope are refused. Active overlapping downloads are
left to Arr recovery. Unsupported player/codec problems receive advice rather
than an endless replacement cycle. No fresh title additions or manual release
research occur. Series scope excludes specials unless explicitly selected.

Nine historical reports were inspected during implementation: missing episodes,
wrong episodes, playback freezes, and one Dolby Vision compatibility complaint.
The latter was previously handled with redownload plus player conversion advice.

After Arr accepts the search, the task posts a deduplicated comment, verifies the
comment and resolved status, and sends Pushover. The comment explicitly says the
replacement download has not been verified. Search completion/download/import is
fire and forget.

A durable per-issue reservation precedes mutation. An interrupted mutation is
not repeated automatically; it becomes a needs-attention outcome. Comment/status
completion can resume without repeating deletion or search. A new human comment
or changed report scope permits another assessment after a completed attempt.
Reopening without changing the report does not repeat a repair. Five new/pending
issues are handled per run, with bounded list/response sizes. An ambiguous
Pushover delivery is reserved rather than sent twice and keeps the task failed
until reconciled. Confirmed HTTP 4xx rejections may retry safely.

Disable the task before investigating a repair manually. Inspect task logs and
Observer comments before adding a new comment to request another attempt.
