/** Ceilings shared by every composer surface (live pane, draft, popup) so a
 *  chip that one surface refuses to stage is refused everywhere. */

/** Total staged attachments allowed on a single turn. */
export const ATTACHMENT_HARD_LIMIT = 20;

/** Conversation handoffs allowed on a single turn. Each one can carry a full
 *  summary (or a budgeted transcript), so a handful is already a large share
 *  of the receiving model's context. */
export const SESSION_ATTACHMENT_LIMIT = 3;
