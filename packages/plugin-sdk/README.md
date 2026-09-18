# @codemux/plugin-sdk

API 1.0.0 for optional CodeMux feature plugins. Authors use TypeScript and Preact
JSX. Code runs in the bundled QuickJS child, never the desktop renderer. This
package is an implementation preview, not a declaration that release gates passed.

Build this package with `npm ci --ignore-scripts && npm run build`, then
`npm pack --ignore-scripts`. Install the resulting tarball into your author project
alongside a packed `@codemux/plugin-cli`. No app checkout is required there.

```tsx
import {definePlugin, Stack, Text, Button} from '@codemux/plugin-sdk';
export default definePlugin({activate(ctx) {
  ctx.commands.register('open', context => ctx.panels.open('brief', context));
  ctx.panels.register('brief', ({context}) => <Stack spacing="md">
    <Text>A separately distributed plugin</Text>
    <Button onPress={event => ctx.composer.appendText(event.context, 'A brief')
      .then(() => {})}>Add to draft</Button>
  </Stack>);
}});
```

Declare each ID in `manifest.json` before registration, and declare
`composer.append` for this example. Each registration returns a disposer. The app
owns final cleanup regardless of author cleanup failures. Settings and storage
are private to an installation; credentials never enter JS. UI events supply
opaque context handles, including expiring single-use user interactions.

All host methods return promises and reject with `PluginError.code`. Handle
`NO_COMPOSER`, `CONTEXT_STALE`, `INTERACTION_REQUIRED`, missing credentials and
network failures in the plugin UI. Never save a context handle in storage. Use the
new handle supplied by workspace changes, commands, or UI events.

The public operation and permission contracts are in the engineering specification
and `dist/types.d.ts`; `schema/manifest.json` is generated from the Rust contract.
The desktop performs semantic validation in addition to schema validation.
Components accept token-based properties, never CSS or arbitrary HTML. Hooks are
Preact hooks. Browser libraries requiring a real browser DOM are unsupported.
