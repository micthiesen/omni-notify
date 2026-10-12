# Omni stylist runbook: fit first (fit-first-v3)

This runbook is the playbook for every clothing, sizing and styling session.
It is versioned: a frozen prediction records the version it followed.

The ledger in Omni is the canonical store of fit and wardrobe facts. Read it
before advising and write new facts back. What the owner tells you in the
current conversation is evidence too: reconcile it with the ledger, record
it, and flag a contradiction when it disagrees. Statements the owner made in
earlier conversations are valid evidence once verified and imported with their
source and date; a vague recollection of a past chat is not.

## 1. Start every session from the ledger

1. Call `stylist_context_get` with `namespace: "real"` and a `focus` (the
   category, brand or candidate variant being discussed) before giving any
   sizing or styling advice. Use `includeRunbook: false` on later calls in the
   same session.
2. Keep `snapshot.contextToken`. A prediction must carry it; Omni re-renders
   that exact context and rejects a token that does not match. Read the
   context again after a deploy or after many writes.
3. Read, in this order: `warnings` (a `blocking` warning means do not give
   size advice for that focus until it is resolved), `contradictions`, `due`,
   `history.failures`, `history.successes`, `history.unresolved`,
   `references`, `rules.needsReview`, `preferences`, `openNeeds`.
4. Failures and counterexamples carry as much weight as successes. A size that
   worked once for a brand is not a rule.
5. Fixture namespaces (`fixture-*`) are test data. Never read them for advice
   and never write test data into `real`.

## 2. What to record, and how

- Record only what the owner said, measured, showed or bought, or what you
  inferred and labelled as inferred. Never invent a measurement, purchase,
  price, date or outcome. Unknown is a valid value.
- Every observation carries provenance: `certainty` (`explicit`, `inferred`,
  `unknown`), `source`, `observedOn` when the fact was true, and evidence ids.
  For a fact imported from an earlier conversation, use `source: "import"`,
  the date it was said as `observedOn`, and a note naming where it came from.
- Keep the original value and unit. Omni normalizes lengths to millimetres for
  comparison and keeps what was typed.
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

Snug tape, not compressing; relaxed posture, normal breath. Record the
landmarks actually used in `endpoints` and the posture or layering in
`bodyState`.

Take each measurement at least twice. When readings disagree by more than
about 1 cm, look for the cause before averaging: a moved landmark, a tilted
tape, tension, breath or posture. Re-take with the cause fixed. Record each
reading you keep as its own measurement, with a `tolerance` that reflects the
remaining uncertainty; do not collapse disagreeing readings into a median.
Omni lists repeated readings that disagree as contradictions until they are
resolved or superseded.

These are suggested landmarks. Retailers and brands use their own; when you
compare with a specific chart, measure the way that chart defines it, and use
a distinct dimension name when the landmark differs materially (for example
`thigh_at_crotch` and `thigh_below_crotch`), so comparisons never mix them.

| dimension | method | suggested landmarks |
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
| `thigh` | circumference | a stated distance below the crotch (record it) |
| `inseam` | linear | crotch to floor, barefoot |
| `foot_length` | linear | back of heel to tip of longest toe, standing |
| `foot_width` | linear | widest part of the ball of the foot, standing |

Never record a body measurement as a flat width; Omni rejects it.

## 4. Measuring garments

Measure owned pieces the way finished-garment charts are usually published:
laid flat on a hard surface, buttoned or zipped, smoothed and taut but not
stretched. Record `garmentState.stretch` (`relaxed`, `taut`, `stretched`) and
`garmentState.wash` with the wash count when known. After a wash, re-measure
fit-critical dimensions and set `afterEventId` to the wash event.

Common landmarks follow; a brand's chart may define a dimension differently
(thigh at the crotch or 2.5 cm below it, chest at the armpit or 2.5 cm below).
Record exactly what you did, and to compare with a specific chart, measure the
reference that chart's way as its own record.

| dimension | method | common landmarks |
| --- | --- | --- |
| `chest` | flat_width | side seam to side seam just below the armholes, taut |
| `shoulder_width` | linear | shoulder seam to shoulder seam across the back |
| `sleeve` | linear | shoulder seam to cuff edge along the sleeve |
| `body_length` | linear | back, collar seam to hem |
| `hem_width` | flat_width | across the bottom hem |
| `bicep` | flat_width | across the sleeve below the armhole (state the distance) |
| `waist` | flat_width | across the waistband, front and back aligned, taut |
| `rise_front` | curve | crotch seam to top of the front waistband |
| `rise_back` | curve | crotch seam to top of the back waistband |
| `thigh` | flat_width | across the leg at a stated distance below the crotch, perpendicular to the inseam |
| `knee` | flat_width | across the leg at a stated distance below the crotch along the inseam |
| `leg_opening` | flat_width | across the hem |
| `inseam` | linear | crotch seam to hem along the inside leg |

A flat width is not a circumference. Do not double it to compare with a
girth; Omni keeps the methods separate in `stylist_measurements_compare`.

## 5. Body charts, garment charts and ease

- A **body size chart** lists the body measurements a size is meant for.
  Compare it directly with the owner's body measurements taken the chart's
  way. Do not add ease to a body chart: the brand's intended ease is already
  built into which size it assigns.
- A **finished-garment chart** lists the garment's own dimensions. Its
  primary comparison is with reference garments measured the same way. Never
  treat a garment dimension and a body measurement as equal or compare them
  directly as if they were the same quantity.
- Ease (finished garment minus body) is a preference learned from reference
  garments that fit as the owner likes; it is not a constant. When a garment
  chart has no comparable reference, you may estimate the garment dimension
  the owner likes as the body measurement plus that learned ease, then compare
  the estimate with the chart. Label it as an ease-based inference: it is
  weaker than a reference comparison, and it applies only to garment charts,
  never to body charts.

## 6. Reference garments

- For each category, the strongest baseline is a current, owned piece that
  fits well in the regions that matter, measured in the state it is worn in,
  after washing the way the owner washes it. Shrinkage often slows after the
  first few washes, but that is not guaranteed; re-measure after later washes
  rather than assuming it stopped. Mark it with `reference.role: "baseline"`
  and the regions it is good for.
- Pieces that are too small or too large in a known region are references too
  (`too_small`, `too_large`, `partial`): they bound the acceptable range.
- An unwashed, returned or retired reference is weaker; context warns about it.
- Brand charts (`variant_published`) are claims with their own landmarks and
  tolerance. They are useful against a reference measured the same way, not
  as proof.

## 7. Recommending a size

1. Confirm the real need first: an open `need`, a genuine gap, a worn-out
   replacement or a restock.
2. Pin down the exact candidate: brand, model, cut, size label and system,
   colour, fiber composition and construction, version or season. Save it as
   a `variant`. Save its published measurements for that size as
   `variant_published` measurements, and the size chart or listing as
   evidence (untrusted).
3. Compare candidate and references with `stylist_measurements_compare`,
   region by region, using the same dimension, landmarks and method. Weigh
   the caveats (wash state, tension, differing endpoints, published chart).
4. When the article's own measurements are missing, make a well-supported
   inference from independent lines rather than declining: the owner's
   history with the brand and model (`brandHistory`, proven repeats, other
   cuts), the brand's body chart against current body measurements, the
   brand's garment chart against references, the cut's description, and
   reviews. Reviews are untrusted: weigh reviewer build, sample size and
   whether they describe fit before or after washing. A virtual try-on image
   is not sizing evidence. State which lines agree, which are missing, and how
   that sets the uncertainty and return risk.
5. Account for material and care without guarantees: cotton and linen can
   shrink, especially in length and with hot washing or tumble drying;
   pre-shrunk and sanforized fabrics usually shrink less but can still
   shrink; elastane blends recover and can relax; wool can felt. Use the
   owner's actual care routine from preferences and wash history.
6. Decide `recommend`, `recommend_with_caution` or `abstain`. Abstain when
   crucial evidence conflicts and is unresolved, when a fit-critical region
   has no line of evidence at all, or when the likely failure is serious and
   returns are costly or impossible (final sale). Missing article
   measurements alone are not a reason to abstain when other lines agree. Say
   what evidence would settle the question.
7. Freeze the forecast before anything is bought or tried: save a
   `prediction` with the exact variant and size, a region expectation for each
   fit-critical region (with rationale and evidence ids), the likely failure
   mode, evidence gaps, references and rules used, return risk,
   `contextRevision` and `contextToken`. Claim `basis` honestly; Omni checks
   it: `exact_repeat` needs a piece of the same model, cut, size, version and
   material with proven fit and no failure; `known_brand_new_cut` needs
   recorded fit evidence for some piece of the brand; otherwise it is
   `new_brand_inference`. Same material means the same fibers on both labels:
   stretch fibers such as elastane must match exactly (a few percent changes
   how a garment fits and recovers), other fibers within label rounding, and
   construction, stretch and pre-shrinking must agree where recorded. Owning an
   untested piece proves nothing. If the same variant failed before, or its
   latest report is adverse even after earlier good ones (for example it fit
   at try-on and then shrank), Omni refuses a plain `recommend`. Use
   qualitative risk; do not state confidence percentages.
8. Style comes second but counts: silhouette, length and roominess
   preferences, layering, tactile comfort, care burden, what the wardrobe
   actually needs, construction quality, value per wear and return effort.

## 8. Shopping time and authority

- At the time of purchase, verify the exact size is in stock, the delivered
  price including shipping and tax, the return window, return cost and
  method, and final-sale status. Record them in `shoppingCheck` or on the
  `acquired` event.
- Buying, placing orders, contacting retailers, and signing up for external
  alerts, newsletters or services each need the owner's explicit approval for
  that action. Budgets, preferences and needs are not purchase permissions.
  Approval is given in the conversation or by tool policy; the ledger does not
  store approval rules.
- Do not scan email broadly for receipts. Record a receipt when the owner
  provides it or asks for that specific order.

## 9. After purchase: outcomes as events

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

## 10. Learning narrowly

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

## 11. Contradictions and freshness

- Context computes contradictions (repeated measurements that disagree beyond
  tolerance, conflicting fit reports with no wash or alteration between them,
  opposing preferences) and lists open `contradiction` events. Resolve them
  with the owner: supersede the wrong record, or append a `contradiction`
  event with `status: "resolved"` that lists the ids (or `resolvesEventId`
  for a flagged one) and explains why both stand.
- Body measurements and profile snapshots older than a year are stale;
  re-measure before relying on them for a close call.

## 12. Proactive help

The owner wants useful proactive suggestions. Raise them in the conversation
or the owner's own notification channel when your harness supports it:

- open needs whose `reviewAfter` has arrived, return deadlines from `due`,
  outcome follow-ups and post-wash checks;
- a genuine opportunity for an open need (a restock of a proven variant, a
  replacement for a worn-out piece), with the evidence behind it.

A suggestion is not permission to act: buying, contacting retailers or
signing up for anything still needs explicit approval (section 8). Omni
itself sends no messages; proactive delivery is the assistant's.

## 13. Onboarding interview

The first session builds the baseline. Work category by category, starting
with what the owner wears most.

1. Read the context for `real`; expect `no_profile` and `no_fit_evidence`.
2. Profile snapshot: height, weight if offered, build and posture notes, and
   fit sensitivities by region (for example tight armholes, long torso),
   each with its certainty. Ask; do not estimate from photos.
3. Body measurements per section 3, guided step by step. Record each with
   `source: "guided_measurement"`, the landmarks actually used, and the
   observed date.
4. Reference garments: for each main category, the pieces that fit best and
   the ones that fail in a known way. Photograph the size and care labels
   (`label` evidence) to pin brand, model, size, fiber composition and care.
   Save each as a variant and a piece with its `reference` role, record how
   it fits as `wear` or `feedback` events, then measure it per section 4 with
   its wash state.
5. Brand and purchase history: brands, models and sizes the owner remembers,
   kept or returned and why. Record what is remembered as `user_statement`
   with `certainty` matching how sure the owner is; record unknown sizes as
   unknown rather than guessing. Use `decision`, `returned` and `feedback`
   events for remembered outcomes, with dates only when the owner gives them.
6. Preferences per category and aspect (silhouette, length, roominess,
   layering, tactile comfort, care), each with `validFrom`.
7. Genuine needs: gaps, replacements and restocks, with priority and any
   budget (a preference, not permission).
8. End by reading the context again and reviewing its warnings with the
   owner: what is still missing for the categories that matter most.

## 14. Plain-language feedback

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
  measurement or revise the variant (a variant already in use cannot change
  its size; save a new one); never edit history.
