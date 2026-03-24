#!/bin/bash
# Patch agent-browser to pass windowsVirtualKeyCode through the stream server
# and browser CDP calls. Without this, special keys (Backspace, Enter, etc.)
# don't work in the browser pane's keyboard input.

STREAM_SERVER="node_modules/agent-browser/dist/stream-server.js"
BROWSER="node_modules/agent-browser/dist/browser.js"

if [ -f "$STREAM_SERVER" ]; then
  # Add windowsVirtualKeyCode to keyboard event relay
  if ! grep -q "windowsVirtualKeyCode" "$STREAM_SERVER"; then
    sed -i 's/modifiers: message.modifiers,$/modifiers: message.modifiers,\n                        windowsVirtualKeyCode: message.windowsVirtualKeyCode,/' "$STREAM_SERVER"
    echo "[patch] stream-server.js: added windowsVirtualKeyCode pass-through"
  fi
fi

if [ -f "$BROWSER" ]; then
  # Add windowsVirtualKeyCode to CDP dispatchKeyEvent call
  if ! grep -q "windowsVirtualKeyCode" "$BROWSER"; then
    sed -i 's/modifiers: params.modifiers ?? 0,$/modifiers: params.modifiers ?? 0,\n            windowsVirtualKeyCode: params.windowsVirtualKeyCode ?? 0,/' "$BROWSER"
    echo "[patch] browser.js: added windowsVirtualKeyCode to CDP call"
  fi
fi
