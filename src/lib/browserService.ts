import { BrowserManager } from 'agent-browser';

export interface BrowserOptions {
    headless?: boolean;
    viewport?: { width: number; height: number };
    sessionName?: string;
}

export interface SnapshotResult {
    tree: string;
    refs: Record<string, { role: string; name: string }>;
}

export class BrowserService {
    private managers: Map<string, BrowserManager> = new Map();
    private screencastCallbacks: Map<string, (data: string) => void> = new Map();

    async spawn(browserId: string, options: BrowserOptions = {}): Promise<void> {
        if (this.managers.has(browserId)) {
            const existing = this.managers.get(browserId)!;
            if (await existing.isLaunched()) {
                return;
            }
        }

        const manager = new BrowserManager();
        
        await manager.launch({
            headless: options.headless ?? true,
            viewport: options.viewport ?? { width: 1280, height: 720 },
        });

        this.managers.set(browserId, manager);
    }

    async navigate(browserId: string, url: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }
        await manager.navigate(url);
    }

    async screenshot(browserId: string): Promise<string> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }
        
        const page = await manager.getPage();
        const screenshot = await page.screenshot({ 
            type: 'png'
        });
        
        const buffer = screenshot as Buffer;
        return `data:image/png;base64,${buffer.toString('base64')}`;
    }

    async snapshot(browserId: string, options: { interactive?: boolean; compact?: boolean } = {}): Promise<SnapshotResult> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const result = await manager.getSnapshot({
            interactive: options.interactive ?? false,
            compact: options.compact ?? false,
        });

        return {
            tree: result.tree,
            refs: result.refs,
        };
    }

    async click(browserId: string, selector: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const locator = manager.getLocator(selector);
        await locator.click();
    }

    async fill(browserId: string, selector: string, value: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const locator = manager.getLocator(selector);
        await locator.fill(value);
    }

    async type(browserId: string, selector: string, text: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const locator = manager.getLocator(selector);
        await locator.type(text);
    }

    async press(browserId: string, key: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const page = await manager.getPage();
        await page.keyboard.press(key);
    }

    async eval(browserId: string, script: string): Promise<string> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const page = await manager.getPage();
        const result = await page.evaluate(script);
        return String(result);
    }

    async getUrl(browserId: string): Promise<string> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        return manager.getUrl();
    }

    async getTitle(browserId: string): Promise<string> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        return manager.getTitle();
    }

    async getConsoleLogs(browserId: string): Promise<Array<{ type: string; text: string }>> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const messages = await manager.getConsoleMessages();
        return messages.map(m => ({ type: m.type, text: m.text }));
    }

    async resizeViewport(browserId: string, width: number, height: number): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        await manager.setViewport(width, height);
    }

    async back(browserId: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const page = await manager.getPage();
        await page.goBack();
    }

    async forward(browserId: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const page = await manager.getPage();
        await page.goForward();
    }

    async reload(browserId: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            throw new Error(`No browser session for ${browserId}`);
        }

        const page = await manager.getPage();
        await page.reload();
    }

    async close(browserId: string): Promise<void> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            return;
        }

        await manager.close();
        this.managers.delete(browserId);
    }

    async isReady(browserId: string): Promise<boolean> {
        const manager = this.managers.get(browserId);
        if (!manager) {
            return false;
        }

        try {
            return await manager.isLaunched() && await manager.hasPages();
        } catch {
            return false;
        }
    }
}

export const browserService = new BrowserService();
