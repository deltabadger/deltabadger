#!/usr/bin/env bash
# Start Deltabadger in background (no console output)
cd "$(dirname "${BASH_SOURCE[0]}")"

# Clean up stale PID file if the process is not running
PIDFILE="$PWD/tmp/pids/server.pid"
if [ -f "$PIDFILE" ]; then
  PID=$(cat "$PIDFILE")
  if ! kill -0 "$PID" 2>/dev/null; then
    rm -f "$PIDFILE"
  fi
fi

# Find and run the app
if [[ "$OSTYPE" == "darwin"* ]]; then
  # macOS - use .app bundle for proper icon
  # Prefer debug bundle for development (release requires production secrets)
  if [ -d "src-tauri/target/debug/bundle/macos/Deltabadger.app" ]; then
    open "src-tauri/target/debug/bundle/macos/Deltabadger.app"
  elif [ -d "src-tauri/target/release/bundle/macos/Deltabadger.app" ]; then
    open "src-tauri/target/release/bundle/macos/Deltabadger.app"
  else
    echo "No .app bundle found. Run './setup.sh' first."
    exit 1
  fi
elif [[ "$OSTYPE" == "linux"* ]]; then
  # Linux - check for AppImage or binary
  APPIMAGE=$(find src-tauri/target/release/bundle/appimage -name "*.AppImage" 2>/dev/null | head -1)
  if [ -n "$APPIMAGE" ] && [ -f "$APPIMAGE" ]; then
    nohup "$APPIMAGE" > /dev/null 2>&1 &
    echo "Deltabadger started in background"
  elif [ -f "src-tauri/target/release/deltabadger" ]; then
    nohup src-tauri/target/release/deltabadger > /dev/null 2>&1 &
    echo "Deltabadger started in background"
  elif [ -f "src-tauri/target/debug/deltabadger" ]; then
    nohup src-tauri/target/debug/deltabadger > /dev/null 2>&1 &
    echo "Deltabadger started in background"
  else
    echo "No build found. Run './setup.sh' first."
    exit 1
  fi
else
  echo "Unsupported OS: $OSTYPE"
  exit 1
fi
