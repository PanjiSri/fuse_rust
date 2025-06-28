#!/bin/bash

#./test.sh > log.log 2>&1

set -e

MOUNT_DIR="/tmp/fuse_mount"
TARGET_DIR="/tmp/target"

cleanup() {
    echo "=== Cleaning up ==="
    if [ ! -z "$FUSELOG_PID" ]; then
        kill "$FUSELOG_PID" 2>/dev/null || true
    fi
    pkill -f fuselog_core || true
    fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    rm -rf "$MOUNT_DIR" "$TARGET_DIR"
    rm -f "/tmp/fuselog.sock" "/tmp/statediff.bin"
}
trap cleanup EXIT

echo "=== Building workspace ==="
cargo clean
cargo build --workspace

echo "=== Setting up directories ==="
cleanup
mkdir -p "$MOUNT_DIR" "$TARGET_DIR"

echo "=== Starting fuselog_core ==="
RUST_LOG=info ./target/debug/fuselog_core "$MOUNT_DIR" &
FUSELOG_PID=$!

echo "=== Waiting for filesystem to mount ==="
i=0
while ! mount | grep -q "$MOUNT_DIR"; do
    if [ $i -ge 10 ]; then
        echo "Error: Filesystem mount timed out." >&2
        exit 1
    fi
    sleep 1
    i=$((i+1))
done
echo "Filesystem mounted successfully."

echo "=== Performing file operations ==="
echo "Hello, Fuselog!" > "$MOUNT_DIR/test1.txt"
mkdir -p "$MOUNT_DIR/subdir"
mkdir "$MOUNT_DIR/empty_dir" 
rmdir "$MOUNT_DIR/empty_dir" 
echo "Subdirectory test" > "$MOUNT_DIR/subdir/test2.txt"
echo "Modified test1.txt" >> "$MOUNT_DIR/test1.txt"
echo "Temporary content" > "$MOUNT_DIR/temp.txt"
rm "$MOUNT_DIR/temp.txt"
echo "This will be truncated" > "$MOUNT_DIR/truncate_test.txt"
truncate -s 5 "$MOUNT_DIR/truncate_test.txt"
echo "Rename this file" > "$MOUNT_DIR/rename_me.txt"
mv "$MOUNT_DIR/rename_me.txt" "$MOUNT_DIR/subdir/renamed.txt"
echo "Original file." > "$MOUNT_DIR/original.txt"
ln "$MOUNT_DIR/original.txt" "$MOUNT_DIR/nickname.txt"
ln -s original.txt "$MOUNT_DIR/symlink.txt"
chmod 777 "$MOUNT_DIR/test1.txt"

echo "=== Getting statediff ==="
sleep 1
./target/debug/get_diff > /tmp/statediff.bin

echo "=== Running fuselog_apply ==="
RUST_LOG=info ./target/debug/fuselog_apply "$TARGET_DIR" --statediff=/tmp/statediff.bin

echo "=== Verifying results ==="

echo -n "Verifying test1.txt content... "
test -f "$TARGET_DIR/test1.txt" || { echo "FAIL: test1.txt missing"; exit 1; }
CONTENT_TEST1=$(cat "$TARGET_DIR/test1.txt")
EXPECTED_TEST1="Hello, Fuselog!
Modified test1.txt"
test "$CONTENT_TEST1" = "$EXPECTED_TEST1" || { echo "FAIL: test1.txt content mismatch. Got: '$CONTENT_TEST1', Expected: '$EXPECTED_TEST1'"; exit 1; }
echo "OK"

echo -n "Verifying subdir/test2.txt content... "
test -f "$TARGET_DIR/subdir/test2.txt" || { echo "FAIL: subdir/test2.txt missing"; exit 1; }
CONTENT_TEST2=$(cat "$TARGET_DIR/subdir/test2.txt")
EXPECTED_TEST2="Subdirectory test"
test "$CONTENT_TEST2" = "$EXPECTED_TEST2" || { echo "FAIL: subdir/test2.txt content mismatch. Got: '$CONTENT_TEST2', Expected: '$EXPECTED_TEST2'"; exit 1; }
echo "OK"

echo -n "Verifying temp.txt deletion... "
test ! -f "$TARGET_DIR/temp.txt" || { echo "FAIL: temp.txt should be deleted"; exit 1; }
echo "OK"

echo -n "Verifying file truncation... "
test -f "$TARGET_DIR/truncate_test.txt" || { echo "FAIL: truncate_test.txt missing"; exit 1; }
TRUNCATED_CONTENT=$(cat "$TARGET_DIR/truncate_test.txt")
test "$TRUNCATED_CONTENT" = "This " || { echo "FAIL: truncation failed"; exit 1; }
echo "OK"

echo -n "Verifying empty_dir removal... "
test ! -e "$TARGET_DIR/empty_dir" || { echo "FAIL: empty_dir should have been removed"; exit 1; }
echo "OK"

echo -n "Verifying file rename... "
test ! -f "$TARGET_DIR/rename_me.txt" || { echo "FAIL: rename_me.txt should not exist after move"; exit 1; }
test -f "$TARGET_DIR/subdir/renamed.txt" || { echo "FAIL: subdir/renamed.txt missing after move"; exit 1; }
RENAMED_CONTENT=$(cat "$TARGET_DIR/subdir/renamed.txt")
test "$RENAMED_CONTENT" = "Rename this file" || { echo "FAIL: renamed.txt content mismatch"; exit 1; }
echo "OK"

echo -n "Verifying hard link... "
test -f "$TARGET_DIR/original.txt" || { echo "FAIL: original.txt missing"; exit 1; }
test -f "$TARGET_DIR/nickname.txt" || { echo "FAIL: nickname.txt missing"; exit 1; }
CONTENT_NICKNAME=$(cat "$TARGET_DIR/nickname.txt")
test "$CONTENT_NICKNAME" = "Original file." || { echo "FAIL: nickname.txt content mismatch"; exit 1; }
echo "OK"

echo -n "Verifying symbolic link... "
test -L "$TARGET_DIR/symlink.txt" || { echo "FAIL: symlink.txt is not a symlink or is missing"; exit 1; }
SYMLINK_TARGET=$(readlink "$TARGET_DIR/symlink.txt")
test "$SYMLINK_TARGET" = "original.txt" || { echo "FAIL: symlink.txt has wrong target. Got: '$SYMLINK_TARGET', Expected: 'original.txt'"; exit 1; }
SYMLINK_OWNER_INFO=$(stat -c "%u:%g" "$TARGET_DIR/symlink.txt")
test "$SYMLINK_OWNER_INFO" = "$(id -u):$(id -g)" || { echo "FAIL: symlink owner is $SYMLINK_OWNER_INFO, expected $(id -u):$(id -g)"; exit 1; }
echo "OK"

echo -n "Verifying file mode (chmod)... "
test -f "$TARGET_DIR/test1.txt" || { echo "FAIL: test1.txt missing for chmod test"; exit 1; }
PERMS=$(stat -c "%a" "$TARGET_DIR/test1.txt")
test "${PERMS: -3}" = "777" || { echo "FAIL: chmod failed, mode is $PERMS, expected 777"; exit 1; }
echo "OK"

echo ""
echo "All tests passed, Yeay!"