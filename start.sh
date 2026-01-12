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
  # macOS
  if [ -d "src-tauri/target/release/bundle/macos/Deltabadger.app" ]; then
    open "src-tauri/target/release/bundle/macos/Deltabadger.app"
    exit 0
  fi
elif [[ "$OSTYPE" == "linux"* ]]; then
  # Linux - check for AppImage or deb install
  if [ -f "src-tauri/target/release/bundle/appimage/deltabadger"*.AppImage ]; then
    nohup src-tauri/target/release/bundle/appimage/deltabadger*.AppImage > /dev/null 2>&1 &
    echo "Deltabadger started in background"
    exit 0
  elif [ -f "src-tauri/target/release/deltabadger" ]; then
    nohup src-tauri/target/release/deltabadger > /dev/null 2>&1 &
    echo "Deltabadger started in background"
    exit 0
  fi
fi

# Fallback to debug build
if [ -f "src-tauri/target/debug/deltabadger" ]; then
  nohup src-tauri/target/debug/deltabadger > /dev/null 2>&1 &
  echo "Deltabadger started in background (dev build)"
else
  echo "No build found. Run './setup.sh' first or 'npm run tauri build'"
fi
