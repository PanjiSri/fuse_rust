#!/bin/bash

set -e

MOUNT_DIR="/tmp/fuse_mount"
TARGET_DIR="/tmp/target"

cleanup() {
    echo "===Cleaning up==="
    kill $FUSELOG_PID 2>/dev/null || true
    fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    rm -rf "$MOUNT_DIR" "$TARGET_DIR"
    rm -f "/tmp/fuselog.sock"
}
trap cleanup EXIT

rm -rf "$MOUNT_DIR" "$TARGET_DIR"
mkdir -p "$MOUNT_DIR" "$TARGET_DIR"
rm -f "/tmp/fuselog.sock"

echo "===Building project==="
cargo build

echo "Starting fuselog_core"
RUST_LOG=info cargo run -p fuselog_core -- "$MOUNT_DIR" &
FUSELOG_PID=$!

sleep 3

echo "===Performing file operations==="

echo "Hello, Fuselog!" > "$MOUNT_DIR/test1.txt"
mkdir -p "$MOUNT_DIR/subdir"
echo "Subdirectory test" > "$MOUNT_DIR/subdir/test2.txt"
echo "Modified test1.txt" >> "$MOUNT_DIR/test1.txt"
echo "Temporary content" > "$MOUNT_DIR/temp.txt"
rm "$MOUNT_DIR/temp.txt"
echo "This will be truncated" > "$MOUNT_DIR/truncate_test.txt"
truncate -s 5 "$MOUNT_DIR/truncate_test.txt"

echo "===Running fuselog_apply==="
RUST_LOG=info cargo run -p fuselog_apply -- "$TARGET_DIR"

echo "===Verifying results==="

test -f "$TARGET_DIR/test1.txt" || { echo "test1.txt missing"; exit 1; }
CONTENT_TEST1=$(cat "$TARGET_DIR/test1.txt")
EXPECTED_TEST1="Hello, Fuselog!
Modified test1.txt"
test "$CONTENT_TEST1" = "$EXPECTED_TEST1" || { echo "test1.txt content mismatch. Got: '$CONTENT_TEST1', Expected: '$EXPECTED_TEST1'"; exit 1; }

test -f "$TARGET_DIR/subdir/test2.txt" || { echo "subdir/test2.txt missing"; exit 1; }
CONTENT_TEST2=$(cat "$TARGET_DIR/subdir/test2.txt")
EXPECTED_TEST2="Subdirectory test"
test "$CONTENT_TEST2" = "$EXPECTED_TEST2" || { echo "subdir/test2.txt content mismatch. Got: '$CONTENT_TEST2', Expected: '$EXPECTED_TEST2'"; exit 1; }

test ! -f "$TARGET_DIR/temp.txt" || { echo "temp.txt should be deleted"; exit 1; }
test -f "$TARGET_DIR/truncate_test.txt" || { echo "truncate_test.txt missing"; exit 1; }

TRUNCATED_CONTENT=$(cat "$TARGET_DIR/truncate_test.txt")
test "$TRUNCATED_CONTENT" = "This " || { echo "truncation failed"; exit 1; }

echo "All tests passed!"