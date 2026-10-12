import { realpathSync } from 'node:fs';
import { mergeConfig } from 'vite';
import base from '../../../vite.config';

// This investigation worktree shares dependencies with the main checkout.
// Allow those assets so the mock renders its intended fonts during measurement.
export default mergeConfig(base, {
  server: { fs: { allow: [process.cwd(), realpathSync('node_modules')] } },
});
