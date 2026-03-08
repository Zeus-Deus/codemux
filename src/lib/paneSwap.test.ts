import { describe, expect, it } from 'vitest';
import type { PaneNodeSnapshot } from '../stores/appState';

type LeafRecord = {
    paneId: string;
    payloadId: string;
};

function terminal(index: number): PaneNodeSnapshot {
    return {
        kind: 'terminal',
        pane_id: `pane-${index}`,
        session_id: `session-${index}`,
        title: `Terminal ${index}`
    };
}

function browser(index: number): PaneNodeSnapshot {
    return {
        kind: 'browser',
        pane_id: `browser-pane-${index}`,
        browser_id: `browser-${index}`,
        title: `Browser ${index}`
    };
}

function split(
    paneId: string,
    direction: 'horizontal' | 'vertical',
    children: PaneNodeSnapshot[],
): PaneNodeSnapshot {
    return {
        kind: 'split',
        pane_id: paneId,
        direction,
        child_sizes: Array.from({ length: children.length }, () => 1 / children.length),
        children,
    };
}

function collectLeaves(node: PaneNodeSnapshot): LeafRecord[] {
    if (node.kind === 'terminal') {
        return [{ paneId: node.pane_id, payloadId: `terminal:${node.session_id}` }];
    }

    if (node.kind === 'browser') {
        return [{ paneId: node.pane_id, payloadId: `browser:${node.browser_id}` }];
    }

    return node.children.flatMap(collectLeaves);
}

function cloneNode(node: PaneNodeSnapshot): PaneNodeSnapshot {
    return structuredClone(node);
}

function replaceLeaf(node: PaneNodeSnapshot, paneId: string, replacement: PaneNodeSnapshot): PaneNodeSnapshot {
    if (node.kind === 'terminal' || node.kind === 'browser') {
        return node.pane_id === paneId ? cloneNode(replacement) : cloneNode(node);
    }

    return {
        ...node,
        child_sizes: [...node.child_sizes],
        children: node.children.map((child) => replaceLeaf(child, paneId, replacement)),
    };
}

function findLeaf(node: PaneNodeSnapshot, paneId: string): PaneNodeSnapshot | null {
    if (node.kind === 'terminal' || node.kind === 'browser') {
        return node.pane_id === paneId ? cloneNode(node) : null;
    }

    for (const child of node.children) {
        const found = findLeaf(child, paneId);
        if (found) {
            return found;
        }
    }

    return null;
}

function swapLeaves(root: PaneNodeSnapshot, sourcePaneId: string, targetPaneId: string): PaneNodeSnapshot {
    const source = findLeaf(root, sourcePaneId);
    const target = findLeaf(root, targetPaneId);

    if (!source || !target) {
        throw new Error('Leaf not found');
    }

    const tempId = '__temp-swap-pane__';
    const tempSource = { ...source, pane_id: tempId };
    const withTemp = replaceLeaf(root, sourcePaneId, tempSource);
    const withSourceAtTarget = replaceLeaf(withTemp, targetPaneId, source);
    return replaceLeaf(withSourceAtTarget, tempId, target);
}

function swapPositions<T>(items: T[], sourceIndex: number, targetIndex: number): T[] {
    const next = [...items];
    [next[sourceIndex], next[targetIndex]] = [next[targetIndex], next[sourceIndex]];
    return next;
}

function assertAllPairSwaps(root: PaneNodeSnapshot) {
    const initialLeaves = collectLeaves(root);

    for (let sourceIndex = 0; sourceIndex < initialLeaves.length; sourceIndex += 1) {
        for (let targetIndex = 0; targetIndex < initialLeaves.length; targetIndex += 1) {
            if (sourceIndex === targetIndex) {
                continue;
            }

            const sourcePaneId = initialLeaves[sourceIndex].paneId;
            const targetPaneId = initialLeaves[targetIndex].paneId;
            const swapped = swapLeaves(root, sourcePaneId, targetPaneId);
            const swappedLeaves = collectLeaves(swapped);

            expect(swappedLeaves.map((leaf) => leaf.payloadId)).toEqual(
                swapPositions(initialLeaves.map((leaf) => leaf.payloadId), sourceIndex, targetIndex),
            );

            const restored = swapLeaves(swapped, sourcePaneId, targetPaneId);
            expect(collectLeaves(restored)).toEqual(initialLeaves);
        }
    }
}

describe('pane swap invariants', () => {
    it('covers every ordered leaf pair across multiple layouts', () => {
        const layouts: PaneNodeSnapshot[] = [
            terminal(1),
            split('pair-root', 'horizontal', [terminal(1), terminal(2)]),
            split('six-root', 'vertical', [
                split('six-row-1', 'horizontal', [terminal(1), terminal(2), terminal(3)]),
                split('six-row-2', 'horizontal', [terminal(4), terminal(5), terminal(6)]),
            ]),
            split('mixed-root', 'horizontal', [
                split('mixed-left', 'vertical', [terminal(1), browser(1)]),
                split('mixed-right', 'vertical', [terminal(2), terminal(3)]),
            ]),
        ];

        for (const layout of layouts) {
            assertAllPairSwaps(layout);
        }
    });
});
