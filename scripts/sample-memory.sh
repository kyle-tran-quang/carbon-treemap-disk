#!/bin/zsh
# Samples the memory footprint of a running "Carbon Disk Treemap" app and its
# WebKit helper processes. Usage: scripts/sample-memory.sh [interval_seconds]
# Footprint (not RSS) is the figure Activity Monitor calls "Memory"; RSS also
# counts shared pages and overstates the cost of the app.
interval=${1:-0}
sample() {
  APP=$(pgrep -f "Carbon Disk Treemap.app/Contents/MacOS" | tail -1)
  [ -z "$APP" ] && APP=$(pgrep -f "target/(debug|release)/carbon-treemap-disk" | tail -1)
  [ -z "$APP" ] && APP=$(pgrep -x "carbon-treemap-disk" | tail -1)
  [ -z "$APP" ] && APP=$(pgrep -f "Carbon Disk Treemap" | grep -v "sample-memory" | tail -1)
  [ -z "$APP" ] && { echo "app not running (start with 'pnpm tauri dev' or open the .app bundle first)"; return 1; }
  ST=$(ps -o lstart= -p "$APP" | awk '{print $4}' | cut -c1-7)
  tot=0; out=""
  for p in $APP $(pgrep -f "com.apple.WebKit"); do
    st=$(ps -o lstart= -p "$p" | awk '{print $4}' | cut -c1-7)
    [ "$st" = "$ST" ] || [ "$p" = "$APP" ] || continue
    n=$(ps -o comm= -p "$p" | sed 's#.*/##; s/com.apple.WebKit.//')
    v=$(footprint "$p" 2>&1 | grep -E "Footprint:" | head -1 | sed -E 's/.*Footprint: ([0-9.]+) (KB|MB|GB).*/\1 \2/')
    mb=$(echo "$v" | awk '{ if ($2=="KB") print $1/1024; else if ($2=="GB") print $1*1024; else print $1 }')
    out="$out $n=$(printf %.0f "$mb")MB"; tot=$(echo "$tot + $mb" | bc)
  done
  echo "$(date +%T) total=$(printf %.0f "$tot")MB |$out"
}
if [ "$interval" = 0 ]; then sample; else while sample; do sleep "$interval"; done; fi
