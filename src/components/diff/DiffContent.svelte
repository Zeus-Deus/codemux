<script lang="ts">
    let { diffText, filePath }: {
        diffText: string;
        filePath: string | null;
    } = $props();

    interface DiffLine {
        type: 'add' | 'del' | 'context' | 'hunk-header';
        content: string;
        oldLine: number | null;
        newLine: number | null;
    }

    function parseDiff(text: string): DiffLine[] {
        if (!text) return [];
        const lines: DiffLine[] = [];
        let oldLine = 0;
        let newLine = 0;

        for (const raw of text.split('\n')) {
            if (raw.startsWith('@@')) {
                const match = raw.match(/@@ -(\d+)/);
                if (match) {
                    oldLine = parseInt(match[1], 10);
                    const newMatch = raw.match(/\+(\d+)/);
                    newLine = newMatch ? parseInt(newMatch[1], 10) : oldLine;
                }
                lines.push({ type: 'hunk-header', content: raw, oldLine: null, newLine: null });
            } else if (raw.startsWith('+')) {
                lines.push({ type: 'add', content: raw.slice(1), oldLine: null, newLine });
                newLine++;
            } else if (raw.startsWith('-')) {
                lines.push({ type: 'del', content: raw.slice(1), oldLine, newLine: null });
                oldLine++;
            } else if (raw.startsWith('diff ') || raw.startsWith('index ') || raw.startsWith('---') || raw.startsWith('+++')) {
                // Skip diff headers
            } else {
                // Context line (starts with space or is empty)
                const content = raw.startsWith(' ') ? raw.slice(1) : raw;
                lines.push({ type: 'context', content, oldLine, newLine });
                oldLine++;
                newLine++;
            }
        }
        return lines;
    }

    const diffLines = $derived(parseDiff(diffText));
</script>

<div class="diff-content">
    {#if !filePath}
        <div class="diff-empty">
            <svg width="28" height="28" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" stroke="currentColor" stroke-width="1.5"/>
                <path d="M14 2v6h6" stroke="currentColor" stroke-width="1.5"/>
            </svg>
            <span>Select a file to view changes</span>
        </div>
    {:else if diffLines.length === 0}
        <div class="diff-empty">
            <svg width="28" height="28" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <span>No changes in this file</span>
        </div>
    {:else}
        <div class="diff-lines">
            {#each diffLines as line}
                <div class="diff-line {line.type}">
                    {#if line.type === 'hunk-header'}
                        <span class="gutter hunk-gutter"></span>
                        <span class="line-text hunk-text">{line.content}</span>
                    {:else}
                        <span class="gutter">
                            <span class="line-num old">{line.oldLine ?? ''}</span>
                            <span class="line-num new">{line.newLine ?? ''}</span>
                        </span>
                        <span class="line-prefix">{line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' '}</span>
                        <span class="line-text">{line.content}</span>
                    {/if}
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .diff-content {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        overflow: auto;
        background: var(--ui-layer-0);
    }

    .diff-empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 10px;
        width: 100%;
        height: 100%;
        color: var(--ui-text-muted);
        font-size: 0.82rem;
    }

    .diff-lines {
        display: flex;
        flex-direction: column;
        font-family: var(--ui-font-mono);
        font-size: 12px;
        line-height: 18px;
        padding: 4px 0;
    }

    .diff-line {
        display: flex;
        min-height: 18px;
        white-space: pre;
    }

    .diff-line.add {
        background: color-mix(in srgb, var(--ui-success) 10%, var(--ui-layer-0) 90%);
    }

    .diff-line.del {
        background: color-mix(in srgb, var(--ui-danger) 10%, var(--ui-layer-0) 90%);
    }

    .diff-line.hunk-header {
        background: var(--ui-layer-2);
        padding: 2px 0;
        margin-top: 4px;
    }

    .diff-line.hunk-header:first-child {
        margin-top: 0;
    }

    .gutter {
        display: flex;
        width: 72px;
        flex-shrink: 0;
        user-select: none;
    }

    .hunk-gutter {
        width: 72px;
    }

    .line-num {
        display: inline-block;
        width: 36px;
        text-align: right;
        padding-right: 8px;
        color: var(--ui-text-muted);
        font-size: 11px;
    }

    .line-prefix {
        display: inline-block;
        width: 16px;
        flex-shrink: 0;
        text-align: center;
        user-select: none;
    }

    .diff-line.add .line-prefix {
        color: var(--ui-success);
    }

    .diff-line.del .line-prefix {
        color: var(--ui-danger);
    }

    .diff-line.context .line-prefix {
        color: var(--ui-text-muted);
    }

    .line-text {
        flex: 1;
        min-width: 0;
        padding-right: 16px;
    }

    .hunk-text {
        color: var(--ui-text-muted);
        font-size: 0.75rem;
        padding: 0 12px;
    }
</style>
