import { describe, expect, it } from 'vitest';
import { collectWorkspaceSessionIds, findActiveSessionId } from './paneTree';
import type { SurfaceSnapshot } from '../stores/types';

describe('paneTree helpers', () => {
    const surface: SurfaceSnapshot = {
        surface_id: 'surface-1',
        title: 'Main',
        active_pane_id: 'pane-2',
        root: {
            kind: 'split',
            pane_id: 'pane-root',
            direction: 'horizontal',
            child_sizes: [0.5, 0.5],
            children: [
                {
                    kind: 'terminal',
                    pane_id: 'pane-1',
                    session_id: 'session-1',
                    title: 'One'
                },
                {
                    kind: 'terminal',
                    pane_id: 'pane-2',
                    session_id: 'session-2',
                    title: 'Two'
                }
            ]
        }
    };

    it('finds the active terminal session from a pane tree', () => {
        expect(findActiveSessionId(surface)).toBe('session-2');
    });

    it('collects terminal session ids from surfaces', () => {
        expect([...collectWorkspaceSessionIds([surface])].sort()).toEqual(['session-1', 'session-2']);
    });
});
