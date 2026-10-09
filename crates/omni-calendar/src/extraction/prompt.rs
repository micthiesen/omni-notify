//! The extraction prompt text, generated verbatim from
//! `src/calendar-events/extraction/extractEvents.ts` (template literal pieces).

/// From the start through the timezone guideline's fixed part.
pub(crate) const HEAD: &str = r#"Extract calendar events from this email that the recipient would want on their personal calendar. Return an empty events array if no actionable events are found.

Guidelines:
- Extract real, scheduled events: appointments, flights, hotel stays, concert tickets, reservations, meetings, building maintenance/shutdowns, move-in/out dates, etc.
- Also extract building/strata notices (water shutdowns, power outages, maintenance windows, fire alarm tests) — these affect the recipient's schedule
- Do NOT extract: terms of service updates, privacy policy changes, or other legal/policy notices
- Do NOT extract: sale deadlines, marketing urgency ("offer expires"), password expiration warnings
- Do NOT extract: package delivery or shipping notifications (these are handled by a separate parcel tracking system)
- Do NOT extract any billing, payment, or subscription event — subscription renewals, recurring or auto-pay charges (e.g. iCloud+, Netflix, a PayPal automatic-payment setup), domain/plan renewals, invoices, or payment due dates — regardless of cadence (monthly, annual, or one-off). These are auto-charged and not actionable. This exclusion is about money charged or owed; genuine action-required deadlines that are NOT about payment (e.g. securing API keys by a date, renewing a passport) should still be extracted. A receipt or confirmation for a real scheduled event (a paid concert ticket, a flight or hotel booking) IS still extractable — put the event on the calendar, just never the payment/charge itself
- For flights: create one event per flight segment (outbound, return, connections)
- For multi-day events (hotel stays, retreats, conferences): create one event spanning the first day to the last (set endDate), not separate events per day
- For notices repeating on a fixed pattern: a notice like "daily 9:00-16:00 from Jul 6 to Jul 13" is ONE event on the first day (startDate the first day, startTime 09:00, endTime 16:00) with recurrence { frequency: "daily", until: the last day }. Do NOT set endDate to the last day of the pattern and do NOT create one event per day; endDate is only for a single continuous stay spanning multiple days
- For appointments: use the appointment time, not the "arrive by" time
- Always extract endTime when a time range is given (e.g. "8:00 a.m. – 5:00 p.m." → startTime 08:00, endTime 17:00). Do not omit the end time
- Infer timezone from location context when not explicitly stated (e.g. JFK airport → America/New_York, a restaurant in London → Europe/London, a hotel in Tokyo → Asia/Tokyo)"#;
/// Timezone clause when the recipient's zone is known; the zone name follows.
pub(crate) const LOCAL_TZ_CLAUSE: &str =
    r#". When there are no geographic clues, use the recipient's local timezone: "#;
/// Timezone clause without a local zone.
pub(crate) const NO_TZ_CLAUSE: &str =
    r#". Only leave timeZone empty if there are no geographic clues at all"#;
/// The remaining guidelines and the action classification, ending with a newline.
pub(crate) const TAIL: &str = r#"
- If only a date is mentioned with no time, set allDay to true
- Extract events even when details are partial — include what's available (e.g. a date in the subject line with no time → allDay event)
- Look for dates in subject lines, headers, and filenames mentioned in the email, not just the body text
- If attachments are included, extract event details from them as well (PDFs, images with text)
- Title should be prefixed with a relevant emoji and be concise and descriptive in Title Case (e.g. "🦷 Dentist Appointment", "✈️ Flight YYZ → YVR", "🎭 Hamilton at Princess of Wales Theatre")
- Set reminderMinutes for events that benefit from advance preparation. Examples: flights/travel (1440 = day before), building shutoffs/maintenance (720 = night before), appointments/reservations (60 = 1 hour). Omit for events where the default 30-minute reminder is fine

Action classification:
- Use "create" for new events not already in the existing events list below. Set eventId to null
- Use "cancel" if the email indicates an existing event has been cancelled, voided, or is no longer happening
- Payment receipts and bills confirm past service — they NEVER cancel an upcoming event. Never emit "cancel" because a payment, receipt, invoice, or billing email mentions an appointment or service
- A "cancel" is only honored when its eventId references an existing event from the list below; a cancel without a valid eventId is skipped
- Use "update" if the email indicates an existing event has been rescheduled, moved, or had details changed (new time, location, etc.)
- For "cancel" and "update", set eventId to the id shown in square brackets next to the existing event you are acting on, WITHOUT the brackets (for [evt_2], use evt_2). Copy it exactly. Only use an id from the list; never invent one. Keep the same title and startDate as that existing event
- If an update fundamentally changes the event (e.g. rebooked to a completely different flight), emit a "cancel" for the old event (with its eventId) and a "create" for the new one
- Do NOT generate "cancel" or "update" for events not in the existing events list
- If an email is just a reminder or confirmation for an existing event with no actual changes (same date, time, location), return an empty events array. Do NOT emit an "update" unless something has actually changed
- For updates, include ALL event fields (startTime, endTime, location, timeZone, etc.), not just the changed ones. The update replaces the entire event
- When in doubt, prefer "create"
"#;
