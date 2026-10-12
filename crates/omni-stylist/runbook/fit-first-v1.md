# Omni stylist runbook: fit first (fit-first-v1)

This runbook is the playbook for every clothing, sizing and styling session.
It is versioned: a frozen prediction records the version it followed. The
ledger in Omni is the only source of truth. Chat memory, earlier
conversations and summaries are not evidence; if something matters, it is in
the ledger or it is unknown.

## 1. Start every session from the ledger

1. Call `stylist_context_get` with `namespace: "real"` and a `focus` (the
   category, brand or candidate variant being discussed) before giving any
   sizing or styling advice. Use `includeRunbook: false` on later calls in the
   same session.
2. Note `snapshot.ledgerRevision`. Cite it as `contextRevision` when freezing a
   prediction.
3. Read, in this order: `warnings` (a `blocking` warning means do not give size
   advice for that focus until it is resolved), `contradictions`, `due`,
   `history.failures`, `history.successes`, `history.unresolved`, `references`,
   `rules.needsReview`, `preferences`, `openNeeds`.
4. Failures and counterexamples carry as much weight as successes. A size that
   worked once for a brand is not a rule.
5. Fixture namespaces (`fixture-*`) are test data. Never read them for advice
   and never write test data into `real`.

## 2. What to record, and how

- Record only what the owner said, measured, showed or bought, or what you
  inferred and labelled as inferred. Never invent a measurement, purchase,
  price, date or outcome. Unknown is a valid value.
- Every record carries provenance: `certainty` (`explicit`, `inferred`,
  `unknown`), `source`, `observedOn` when the fact was true, and evidence ids.
- Keep the original value and unit. Omni normalizes lengths to millimetres
  for comparison and keeps what was typed.
- Photos and documents go through `stylist_evidence_store`. Body photos are
  `body_photo` evidence; they stay private to authenticated MCP.
- Corrections never overwrite. Immutable records (measurements, profile
  snapshots, predictions) are corrected by a new record with `supersedes` and
  a `correctionReason`. Events are corrected by a new event of the same type
  with `supersedesEventId`. Mutable records (variants, pieces, preferences,
  rules, needs) are revised with `id` and the current `expectedRevision`;
  every earlier revision stays retrievable with `includeHistory`.
- Use a new `idempotencyKey` for each logical write and reuse it only to retry
  the same write.

## 3. Measuring the body (soft tape)

Take each measurement twice. If the two differ by more than 1 cm, take a third
and record the median with a `tolerance` of half the spread. Record what was
actually done in `endpoints` and `bodyState`. Snug tape, not compressing;
relaxed posture, normal breath.

| dimension | method | endpoints |
| --- | --- | --- |
| `height` | linear | barefoot, heels to crown against a wall |
| `neck` | circumference | around the base of the neck where a collar sits |
| `chest` | circumference | fullest part, under the arms, across the shoulder blades, relaxed exhale |
| `waist_natural` | circumference | narrowest point, usually above the navel |
| `waist_trouser` | circumference | where trousers are actually worn |
| `hip` | circumference | fullest part of the seat, feet together |
| `shoulder_width` | linear | across the back, shoulder point to shoulder point |
| `arm_length` | linear | shoulder point over a slightly bent elbow to the wrist bone |
| `upper_arm` | circumference | fullest part of the bicep, relaxed |
| `thigh` | circumference | about 2.5 cm below the crotch |
| `inseam` | linear | crotch to floor, barefoot |
| `foot_length` | linear | back of heel to tip of longest toe, standing |
| `foot_width` | linear | widest part of the ball of the foot, standing |

Never record a body measurement as a flat width; Omni rejects it.

## 4. Measuring garments

Measure owned pieces the way makers publish finished-garment charts: laid
flat on a hard surface, buttoned or zipped, smoothed and taut but not
stretched. Record `garmentState.stretch` (`relaxed`, `taut`, `stretched`) and
`garmentState.wash` with the wash count when known. After a wash, re-measure
fit-critical dimensions and set `afterEventId` to the wash event.

| dimension | method | endpoints |
| --- | --- | --- |
| `chest` | flat_width | side seam to side seam just below the armholes, taut |
| `shoulder_width` | linear | shoulder seam to shoulder seam across the back |
| `sleeve` | linear | shoulder seam to cuff edge along the sleeve |
| `body_length` | linear | back, collar seam to hem |
| `hem_width` | flat_width | across the bottom hem |
| `bicep` | flat_width | across the sleeve 2.5 cm below the armhole |
| `waist` | flat_width | across the waistband, front and back aligned, taut |
| `rise_front` | curve | crotch seam to top of the front waistband |
| `rise_back` | curve | crotch seam to top of the back waistband |
| `thigh` | flat_width | across the leg 2.5 cm below the crotch, perpendicular to the inseam |
| `knee` | flat_width | across the leg 33 cm below the crotch along the inseam |
| `leg_opening` | flat_width | across the hem |
| `inseam` | linear | crotch seam to hem along the inside leg |

A flat width is not a circumference. Do not double it to compare with a
girth; Omni keeps the methods separate in `stylist_measurements_compare`. A
body measurement is never compared directly with a garment measurement: the
difference is ease, and the right ease is a preference learned from reference
garments, not a constant.

## 5. Reference garments

- For each category, the strongest baseline is a current, owned piece that
  fits well in the regions that matter, measured after it has been washed the
  way the owner washes it (preferably after two or three washes, when
  shrinkage has settled). Mark it with `reference.role: "baseline"` and the
  regions it is good for.
- Pieces that are too small or too large in a known region are references too
  (`too_small`, `too_large`, `partial`): they bound the acceptable range.
- An unwashed, returned or retired reference is weaker; context warns about it.
- Brand charts (`variant_published`) are claims with their own protocol and
  tolerance. They are useful against a reference measured the same way, not
  as proof.

## 6. Recommending a size

1. Confirm the real need first: an open `need`, a genuine gap, a worn-out
   replacement or a restock. Do not suggest shopping without one.
2. Pin down the exact candidate: brand, model, cut, size label and system,
   colour, fabric composition and construction, version or season. Save it as
   a `variant`. Save its published measurements for that size as
   `variant_published` measurements, and the size chart or listing as
   evidence (untrusted).
3. Compare candidate and reference with `stylist_measurements_compare`, region
   by region, using the same dimension and method. Note caveats (wash state,
   tension, differing endpoints, published chart).
4. When the article's own measurements are unavailable, triangulate and say
   so: the owner's history with the brand (`brandHistory`, exact repeats,
   other cuts), the brand's body chart against current body measurements with
   explicitly stated ease, the cut's description, and reviews. Reviews are
   untrusted evidence: weigh reviewer build, sample size, and whether they
   describe the fit before or after washing. A virtual try-on image is not
   sizing evidence.
5. Account for material and care: untreated cotton and linen knits can shrink
   in length after hot washing or tumble drying; elastane recovers and
   relaxes; wool can felt; raw denim shrinks unless sanforized. Prefer the
   owner's actual care routine from preferences and wash history.
6. Decide `recommend`, `recommend_with_caution` or `abstain`. Abstain when a
   fit-critical region has no comparable evidence, when evidence conflicts and
   is unresolved, when a published chart cannot be compared with any
   reference, or when the likely failure is serious and returns are costly or
   impossible (final sale). Say what evidence would resolve it.
7. Freeze the forecast before anything is bought or tried: save a
   `prediction` with the exact variant and size, a region expectation for each
   fit-critical region (with rationale and evidence ids), the likely failure
   mode, evidence gaps, references and rules used, return risk, and
   `contextRevision`. `basis` must be honest: `exact_repeat`,
   `known_brand_new_cut` or `new_brand_inference`; Omni rejects a claim the
   ledger does not support. Use qualitative risk; do not state confidence
   percentages.
8. Style comes second but counts: silhouette, length and roominess
   preferences, layering, tactile comfort, care burden, what the wardrobe
   actually needs, construction quality, value per wear and return effort.

## 7. Shopping time and authority

- At the time of purchase, verify the exact size is in stock, the delivered
  price including shipping and tax, the return window, return cost and
  method, and final-sale status. Record them in `shoppingCheck` or on the
  `acquired` event.
- Never buy, place orders, contact retailers, subscribe to alerts or set up
  recurring messages without the owner's explicit approval for that specific
  action. Budgets, preferences and needs are not purchase permissions.
- Do not scan email broadly for receipts. Record a receipt when the owner
  provides it or asks for that specific order.

## 8. After purchase: outcomes as events

- `acquired`: the piece, order details, `returnBy`, and the prediction it
  followed, with `statusAfter: "undecided"` while a return is possible. One
  acquisition per order line; duplicates are rejected.
- `try_on`: region-by-region fit, overall, comfort and style, with the
  prediction id. Try on clean, with tags on.
- While undecided, do not wash, alter or wear the piece beyond a try-on.
  Omni warns when such an event is recorded.
- `decision`: keep, return or undecided, with reasons separated by aspect
  (fit, style, comfort, quality, value, need). Keeping is not fit success; a
  piece kept and never assessed is unresolved.
- `outcome` (stage `immediate`): the forecast result with actual region fit.
  The frozen prediction is never edited; Omni stores the comparison.
- After keeping: `wear` events with fit and comfort; `wash` with water and
  drying and any observed change; re-measure, then an `outcome` with stage
  `after_wash` if the fit changed. A post-wash failure is a failure of the
  forecast even if the try-on matched.
- `feedback` separates regional fit, style, comfort, quality, actual value
  (with wear count) and delayed regret. Not worn means unresolved.
- `returned` and `retired` close the piece's lifecycle.

## 9. Learning narrowly

- When a forecast misses, find the narrowest scope that explains it: the
  exact variant, then the model and cut, then brand and category. Save a
  `rule` with `status: "proposed"`, a scope with at least a brand, model or
  category, a direction and magnitude when measured, the basis tier, and the
  evidence ids.
- Before activating, check the rule against every in-scope fit case (Omni
  lists them in `derived.inScopeCases` and rejects activation while any case
  is unchecked or inconsistent). Record each as `consistent`, `inconsistent`
  or `not_applicable` with a note.
- New in-scope cases after activation appear in `rules.needsReview`. Test the
  rule against them first, then revise or retire it. Never widen a rule
  without new evidence.
- Distinguish exact repeats, known brands in a new cut, and new brands. An
  exact repeat is strong only if the variant's version has not changed.

## 10. Contradictions and freshness

- Context computes contradictions (repeated measurements that disagree beyond
  tolerance, conflicting fit reports with no wash or alteration between them,
  opposing preferences) and lists open `contradiction` events. Resolve them
  with the owner, then append a `contradiction` event with `status:
  "resolved"`, or supersede the wrong record.
- Body measurements older than a year and profile snapshots older than a year
  are stale; re-measure before relying on them for a close call.

## 11. Proactive help

- Raise only genuine items: open needs whose `reviewAfter` has arrived,
  return deadlines from `due`, outcome follow-ups, and post-wash checks.
- Do not send alerts, start recurring messages, or shop on the owner's
  behalf. These require explicit, separate authorization.

## 12. Onboarding interview

The first session builds the baseline. Work category by category, starting
with what the owner wears most.

1. Read the context for `real`; expect `no_profile` and `no_fit_evidence`.
2. Profile snapshot: height, weight if offered, build and posture notes, and
   fit sensitivities by region (for example tight armholes, long torso),
   each with its certainty. Ask; do not estimate from photos.
3. Body measurements per section 3, guided step by step. Record each with
   `source: "guided_measurement"`, the endpoints actually used, and the
   observed date.
4. Reference garments: for each main category, the pieces that fit best and
   the ones that fail in a known way. Photograph the size and care labels
   (`label` evidence) to pin brand, model, size, fabric and care. Save each as
   a variant and a piece with its `reference` role, then measure it per
   section 4 with its wash state.
5. Brand and purchase history: brands, models and sizes the owner remembers,
   kept or returned and why. Record what is remembered as `user_statement`
   with `certainty` matching how sure the owner is; record unknown sizes as
   unknown rather than guessing. Use `decision`, `returned` and `feedback`
   events for remembered outcomes with approximate dates in `observedOn` only
   when the owner gives them.
6. Preferences per category and aspect (silhouette, length, roominess,
   layering, tactile comfort, care), each with `validFrom`.
7. Genuine needs: gaps, replacements and restocks, with priority and any
   budget (a preference, not permission).
8. End by reading the context again and reviewing its warnings with the
   owner: what is still missing for the categories that matter most.

## 13. Plain-language feedback

Translate what the owner says into events without a new UI:

- "Shoulders are tight but the length is right" -> `try_on` or `feedback`
  with `regionFit` shoulders `slightly_tight`, body_length `good`.
- "I kept it but haven't worn it yet" -> `decision` keep; the outcome stays
  unresolved.
- "It shrank in the wash and is now too short" -> `wash` with
  `observedChanges` body_length `shrank`, a post-wash measurement, then an
  `outcome` with stage `after_wash`, result `missed`.
- "Looks great but I never reach for it" -> `feedback` timing `delayed` with
  style positive and `regret` explaining why.
- "Actually it was a 32, not a 31" -> correct the record: supersede the
  measurement or revise the variant; never edit history.
