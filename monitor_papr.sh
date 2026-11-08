#!/usr/bin/env bash

LOG_FILE="papr_ram_usage.txt"

kitty ./target/release/papr &
KITTY_PID=$!

PAPR_PID=$(pgrep -n -f 'target/release/papr')

echo "Timestamp,RAM_MB" > "$LOG_FILE"
echo "Started papr in Kitty (PID: $PAPR_PID)"
echo "Monitoring RAM usage every 0.5 seconds..."

while kill -0 "$PAPR_PID" 2>/dev/null; do
    RAM=$(ps -o rss= -p "$PAPR_PID" | awk '{printf "%.2f", $1/1024}')
    TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S.%3N')
    [[ -n "$RAM" ]] && echo "$TIMESTAMP,$RAM" >> "$LOG_FILE"
    sleep 0.5
done

echo "Papr has exited. Monitoring stopped."
