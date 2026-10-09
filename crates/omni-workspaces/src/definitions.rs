//! The two workspaces (`src/workspaces/definitions.ts`); instruction strings verbatim.

use omni_api::workspaces::{
    WorkspaceArtifactDefinition, WorkspaceArtifactKind, WorkspaceDefinition,
};

fn artifact(
    key: &str,
    title: &str,
    kind: WorkspaceArtifactKind,
    instructions: &str,
) -> WorkspaceArtifactDefinition {
    WorkspaceArtifactDefinition {
        key: key.to_owned(),
        title: title.to_owned(),
        kind,
        instructions: instructions.to_owned(),
    }
}

/// `Purchase Research`; `schedule` is `WORKSPACE_SCHEDULE`.
pub fn purchase_research(schedule: &str) -> WorkspaceDefinition {
    WorkspaceDefinition {
        id: "purchase-research".to_owned(),
        title: "Purchase Research".to_owned(),
        description: "Researches purchases under consideration, maintains evidence-backed comparisons, and follows decisions through delivery and return deadlines.".to_owned(),
        subject_label: "Purchase".to_owned(),
        subject_label_plural: "Purchases".to_owned(),
        task_name: "PurchaseResearch".to_owned(),
        schedule: schedule.to_owned(),
        scheduled_runs: None,
        input_placeholder: Some("What are you looking to buy, compare, or keep watching?".to_owned()),
        follow_up_placeholder: Some("Ask a follow-up, change requirements, add a candidate, or request fresh research…".to_owned()),
        instructions: r#"You maintain purchase dossiers for one person. A dossier begins when the user says they are considering or tracking something and ends when it is completed or archived.

Be practical and skeptical. Preserve the user's actual requirements, separate facts from recommendations, cite current sources, record why candidates were rejected, and call out unanswered questions. Never purchase anything, send messages, broaden email access, or write to the calendar directly. Those effects must be emitted as reviewable proposals.

Scheduled runs should work only on active dossiers with unresolved questions, stale evidence, meaningful market changes, or a time-sensitive deadline. Do not manufacture updates merely to appear busy. Notifications are for material changes only.

When the user explicitly asks to watch email, propose a narrow email scope using exact senders/domains and product-specific keywords. Never propose an empty or catch-all scope. When a confirmed purchase has a return or price-adjustment deadline, propose a calendar reminder with enough lead time to act."#.to_owned(),
        artifacts: vec![
            artifact(
                "brief",
                "Brief",
                WorkspaceArtifactKind::Markdown,
                "The current objective, budget, timing, must-haves, preferences, constraints, and explicit non-goals.",
            ),
            artifact(
                "requirements",
                "Requirements",
                WorkspaceArtifactKind::Structured,
                "A concise Markdown checklist grouped into required, preferred, and unresolved requirements.",
            ),
            artifact(
                "comparison",
                "Comparison",
                WorkspaceArtifactKind::Structured,
                "A Markdown comparison table of serious candidates, current price when known, evidence-backed strengths, weaknesses, and status.",
            ),
            artifact(
                "research",
                "Research",
                WorkspaceArtifactKind::EvidenceLedger,
                "Dated findings with source links, freshness notes, disagreements between sources, and facts that need verification.",
            ),
            artifact(
                "questions",
                "Open Questions",
                WorkspaceArtifactKind::Collection,
                "The smallest useful list of questions whose answers could change the decision.",
            ),
            artifact(
                "decision",
                "Decision",
                WorkspaceArtifactKind::Timeline,
                "Decision history, rejected candidates and why, final purchase details, delivery state, and return or price-adjustment deadlines.",
            ),
        ],
    }
}

/// `Marketplace Selling`; `schedule` is `WORKSPACE_SCHEDULE`.
pub fn marketplace_selling(schedule: &str) -> WorkspaceDefinition {
    WorkspaceDefinition {
        id: "marketplace-selling".to_owned(),
        title: "Marketplace Selling".to_owned(),
        description: "Turns item details into complete Facebook Marketplace listings, researches realistic prices, plans photos, and tracks each sale from draft to pickup.".to_owned(),
        subject_label: "Item".to_owned(),
        subject_label_plural: "Items".to_owned(),
        task_name: "MarketplaceSelling".to_owned(),
        schedule: schedule.to_owned(),
        scheduled_runs: Some(false),
        input_placeholder: Some("What do you want to sell? Share whatever you know, even if it is incomplete.".to_owned()),
        follow_up_placeholder: Some("Add details or photos, revise the listing, check the price, record an offer, or plan the next step…".to_owned()),
        instructions: r#"You help one person prepare and manage items for sale on Facebook Marketplace. Each subject is one item or one logical lot. The workspace is strictly on demand: inactivity is normal. Never manufacture work, chase the user, or interpret silence as a problem.

Turn incomplete user input into steady progress. Preserve confirmed facts, clearly label assumptions, and ask only the smallest useful questions. Produce ready-to-paste listing fields, not generic selling advice. Never publish or edit a Facebook listing, contact a buyer, accept an offer, disclose a home address, or arrange a meeting. The user performs those actions.

Meta's public help consistently identifies photos or video, title, price, and category as core listing inputs, but the exact form varies by category, device, account, and region. Maintain the common fields too: listing type, condition, description, location, availability or quantity, delivery or pickup method, brand/model, product tags, and category-specific attributes. Mark each field Confirmed, Drafted, Missing, or Not Applicable. If the current Marketplace form contains unfamiliar required fields, ask the user for a screenshot or the field labels and update the dossier instead of guessing.

For pricing, research current local and broader-market comparables when useful. Separate asking prices from credible sold-price evidence, adjust for condition, completeness, age, seasonality, and local demand, and record the date and source. Recommend a list price, expected sale range, and private walk-away price. Never reveal the walk-away price in public listing copy.

Draft concise, natural titles and descriptions. State material flaws plainly, avoid unsupported claims, and do not use spammy keyword stuffing. Build a photo checklist that shows the whole item, identifying details, included accessories, scale, operation when relevant, and every disclosed flaw. Help evaluate pasted buyer messages and offers, but keep negotiation and meetup decisions with the user.

Do not propose email scopes or calendar events unless the user explicitly asks for them. Notifications should only accompany a user-triggered approval proposal; never notify merely because an item has been inactive."#.to_owned(),
        artifacts: vec![
            artifact(
                "item-details",
                "Item Details",
                WorkspaceArtifactKind::Structured,
                "The source of truth for identity, brand/model, dimensions, age, ownership, included parts, working state, condition, flaws, repairs, and facts still needing confirmation.",
            ),
            artifact(
                "listing-fields",
                "Listing Fields",
                WorkspaceArtifactKind::Structured,
                "A ready-to-paste Facebook Marketplace field sheet. Include listing type, photos/video readiness, title, price, category, condition, description, location, availability/quantity, fulfillment method, tags, and category-dependent attributes. Mark every field Confirmed, Drafted, Missing, or Not Applicable.",
            ),
            artifact(
                "pricing",
                "Pricing",
                WorkspaceArtifactKind::EvidenceLedger,
                "Dated comparable listings and sold evidence with source, condition adjustments, list-price recommendation, expected sale range, negotiation room, and a clearly private walk-away price.",
            ),
            artifact(
                "photos",
                "Photo Plan",
                WorkspaceArtifactKind::Collection,
                "An ordered photo and optional video checklist, including cover choice, identifying details, accessories, scale, proof of operation when useful, and clear shots of every flaw.",
            ),
            artifact(
                "progress",
                "Progress",
                WorkspaceArtifactKind::Timeline,
                "A compact next-action checklist and history from intake through photos, draft, published, offers, pending, sold, pickup/shipping, and completion. Inactivity requires no action by itself.",
            ),
            artifact(
                "buyer-plan",
                "Buyer and Handoff Plan",
                WorkspaceArtifactKind::Markdown,
                "Private negotiation guidance, common-answer snippets, offer log, pickup or delivery constraints, payment preference, safety considerations, and handoff checklist. Never place private addresses or the walk-away price in public copy.",
            ),
        ],
    }
}

/// Every workspace, in TS order.
pub fn workspace_definitions(schedule: &str) -> Vec<WorkspaceDefinition> {
    vec![purchase_research(schedule), marketplace_selling(schedule)]
}

/// Whether scheduled refreshes run (`scheduledRuns !== false`).
pub fn scheduled_runs(definition: &WorkspaceDefinition) -> bool {
    definition.scheduled_runs != Some(false)
}
