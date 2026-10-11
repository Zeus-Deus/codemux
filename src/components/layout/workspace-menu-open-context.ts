import { createContext } from "react";

/**
 * True while the row's right-click menu is open. `WorkspaceInboxMenu`
 * provides it and `WorkspaceHoverCard` reads it, so the hover preview never
 * opens over the menu or pops back up when focus returns to the row.
 */
export const WorkspaceMenuOpenContext = createContext(false);
