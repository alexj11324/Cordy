#!/bin/sh
set -eu

echo "Running database migrations..."
./migrate up &
migration_pid=$!

forward_migration_signal() {
  signal="$1"
  kill "-$signal" "$migration_pid" 2>/dev/null || true
}

trap 'forward_migration_signal TERM' TERM
trap 'forward_migration_signal INT' INT

migration_status=0
wait "$migration_pid" || migration_status=$?
# A trapped signal interrupts the shell's wait before the child has necessarily
# finished its own graceful shutdown. Reap it before PID 1 exits.
if kill -0 "$migration_pid" 2>/dev/null; then
  wait "$migration_pid" || migration_status=$?
fi
trap - TERM INT

if [ "$migration_status" -ne 0 ]; then
  exit "$migration_status"
fi

echo "Starting server..."
exec ./server
