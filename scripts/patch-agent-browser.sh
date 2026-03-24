#!/bin/bash
# Patch agent-browser for Codemux browser pane support.
# Applied automatically via npm postinstall.

STREAM_SERVER="node_modules/agent-browser/dist/stream-server.js"
BROWSER="node_modules/agent-browser/dist/browser.js"

if [ -f "$STREAM_SERVER" ]; then
  # 1. Pass windowsVirtualKeyCode through keyboard event relay
  if ! grep -q "windowsVirtualKeyCode" "$STREAM_SERVER"; then
    sed -i 's/modifiers: message.modifiers,$/modifiers: message.modifiers,\n                        windowsVirtualKeyCode: message.windowsVirtualKeyCode,/' "$STREAM_SERVER"
    echo "[patch] stream-server.js: added windowsVirtualKeyCode pass-through"
  fi

  # 2. Add dynamic screencast dimensions (screencastWidth/screencastHeight properties)
  if ! grep -q "screencastWidth" "$STREAM_SERVER"; then
    # Add properties after isScreencasting
    sed -i 's/isScreencasting = false;/isScreencasting = false;\n    screencastWidth = 1280;\n    screencastHeight = 720;/' "$STREAM_SERVER"
    # Use dynamic dimensions in startScreencast
    sed -i 's/maxWidth: 1280/maxWidth: this.screencastWidth/' "$STREAM_SERVER"
    sed -i 's/maxHeight: 720/maxHeight: this.screencastHeight/' "$STREAM_SERVER"
    echo "[patch] stream-server.js: added dynamic screencast dimensions"
  fi

  # 3. Add "resize" message handler to resize viewport + restart screencast
  if ! grep -q "'resize'" "$STREAM_SERVER"; then
    sed -i "/case 'status':/i\\
                case 'resize':\\
                    this.screencastWidth = message.width || 1280;\\
                    this.screencastHeight = message.height || 720;\\
                    if (this.browser.isLaunched()) {\\
                        await this.browser.setViewport(this.screencastWidth, this.screencastHeight);\\
                        if (this.isScreencasting) {\\
                            await this.stopScreencast();\\
                        }\\
                        await this.startScreencast();\\
                    }\\
                    break;" "$STREAM_SERVER"
    echo "[patch] stream-server.js: added resize message handler with viewport sync"
  fi
fi

if [ -f "$BROWSER" ]; then
  # 4. Pass windowsVirtualKeyCode to CDP dispatchKeyEvent call
  if ! grep -q "windowsVirtualKeyCode" "$BROWSER"; then
    sed -i 's/modifiers: params.modifiers ?? 0,$/modifiers: params.modifiers ?? 0,\n            windowsVirtualKeyCode: params.windowsVirtualKeyCode ?? 0,/' "$BROWSER"
    echo "[patch] browser.js: added windowsVirtualKeyCode to CDP call"
  fi
fi
