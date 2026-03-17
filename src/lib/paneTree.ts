import type { PaneNodeSnapshot, SurfaceSnapshot } from '../stores/types';

export function findActiveSessionId(surface: SurfaceSnapshot | null | undefined): string | null {
    const root = surface?.root;
    if (!root || !surface) {
        return null;
    }

    const stack: PaneNodeSnapshot[] = [root];
    while (stack.length > 0) {
        const node = stack.pop();
        if (!node) {
            continue;
        }
        if (node.kind === 'terminal' && node.pane_id === surface.active_pane_id) {
            return node.session_id;
        }
        if (node.kind === 'split') {
            stack.push(...node.children);
        }
    }

    return null;
}

export function collectWorkspaceSessionIds(surfaceList: SurfaceSnapshot[]): Set<string> {
    const sessionIds = new Set<string>();

    for (const surface of surfaceList) {
        const stack: PaneNodeSnapshot[] = [surface.root];
        while (stack.length > 0) {
            const node = stack.pop();
            if (!node) {
                continue;
            }
            if (node.kind === 'terminal') {
                sessionIds.add(node.session_id);
            }
            if (node.kind === 'split') {
                stack.push(...node.children);
            }
        }
    }

    return sessionIds;
}

export function collectLeafPaneIds(root: PaneNodeSnapshot): string[] {
    const paneIds: string[] = [];
    const stack: PaneNodeSnapshot[] = [root];

    while (stack.length > 0) {
        const node = stack.pop();
        if (!node) {
            continue;
        }

        if (node.kind === 'split') {
            stack.push(...node.children);
            continue;
        }

        paneIds.push(node.pane_id);
    }

    return paneIds;
}
