# Recommendation system review checkpoint

Revisit these decisions after at least **20 delivered recommendations** and enough time for outcomes to mature. Prefer waiting for **5 or more explicit ratings** and several watched, ignored, or abandoned outcomes. Until then, the sample is too small for trustworthy automatic optimization.

## Evaluate with real outcomes

- Compare acceptance and completion rates by candidate source, genre, medium, runtime, and series commitment. Only then consider learned source or feature weights.
- Audit `ignored` recommendations manually. Decide whether they represent weak picks, bad timing, an already-large queue, or simply insufficient observation time.
- Review research coverage and failures. Add another research source only if finalist decisions repeatedly lack a specific kind of evidence.
- Decide whether to add a separate “already in Plex” recommendation lane. Measure whether local availability materially improves starts and completions first.
- Review recommendation frequency and `no_add` frequency. Change the schedule only if the queue or notification load is consistently wrong.
- Consider optional feedback reasons after seeing which ambiguity actually matters. Keep the one-tap feedback path.
- Evaluate a bounded follow-up research/tool loop for close decisions. Add it only if stored decisions show the single-pass selector is missing correctable facts.
- Build success-rate charts once there are enough outcomes to avoid presenting noise as insight.

## Reflection quality audit

- Inspect every taste-profile claim and its cited evidence. Remove prompt rules that permit unsupported or overly broad claims.
- Check that recent saturation does not overwrite stable preferences, and that the profile retains deliberate exploration targets.
- Compare the profile’s predictions with subsequent starts, completions, rewatches, and explicit feedback.
- Verify that one unusual watch cannot dominate the profile and that explicit negative feedback is not generalized beyond its evidence.
- Reassess movie, limited-series, and long-series commitment preferences using actual completion behavior.

## Deliberately deferred

- No vector database or custom recommendation model. The evidence volume does not justify either yet.
- No real-time profile rebuild. Weekly checkpointed reflection is cheaper and less reactive.
- No autonomous prompt, code, or scoring-rule rewriting. Reflection may update versioned taste context, while application behavior remains reviewable code.
- No JMAP-style Plex checkpoint protocol. Bounded paginated polling is simple, reliable, and proportionate to this library.
