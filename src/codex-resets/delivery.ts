import { createResetDelivery } from "../reset-alerts/delivery.js";

export { ResetAlertDeliveryError, type ResetAlert } from "../reset-alerts/delivery.js";

// Keep the existing entity namespace and alert keys across the shared-code move.
export const { deliverResetAlerts, ResetDeliveryEntity } = createResetDelivery("codex");
