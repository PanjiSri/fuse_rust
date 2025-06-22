#!/bin/bash

#./sqlite.sh > log.log 2>&1

set -e

MOUNT_DIR="/tmp/fuse_mount_sql"
TARGET_DIR="/tmp/target_sql"
DB_NAME="test.db"

cleanup() {
    echo "=== Cleaning up ==="
    if [ ! -z "$FUSELOG_PID" ]; then
        sudo kill "$FUSELOG_PID" 2>/dev/null || true
        sleep 1
    fi
    sudo pkill -f fuselog_core || true
    fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    sudo rm -rf "$MOUNT_DIR" "$TARGET_DIR"
    sudo rm -f "/tmp/fuselog.sock"
}
trap cleanup EXIT

echo "=== Building workspace (if needed) ==="
cargo clean
cargo build --workspace

echo "=== Setting up directories ==="
cleanup
mkdir -p "$MOUNT_DIR" "$TARGET_DIR"
sudo chown "$USER:$USER" "$MOUNT_DIR" "$TARGET_DIR"

echo "=== Starting fuselog_core ==="
sudo RUST_LOG=info,fuselog_core=debug ./target/debug/fuselog_core "$MOUNT_DIR" &
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
echo "Filesystem mounted successfully. PID: $FUSELOG_PID"

echo "=== Performing SQLite operations on FUSE mount ==="
DB_PATH="$MOUNT_DIR/$DB_NAME"

# 1. Create a table and insert initial data
echo "1. Creating table and inserting two records..."
sqlite3 "$DB_PATH" "CREATE TABLE contacts (id INTEGER PRIMARY KEY, name TEXT, email TEXT);"
sqlite3 "$DB_PATH" "INSERT INTO contacts (name, email) VALUES ('dummy1', 'dummy1@gmail.com');"
sqlite3 "$DB_PATH" "INSERT INTO contacts (name, email) VALUES ('dummy2', 'dummy2@gmail.com');"

# 2. Update a record
echo "2. Updating a record..."
sqlite3 "$DB_PATH" "UPDATE contacts SET email = 'dummy2_new@gmail.com' WHERE name = 'dummy2';"

# 3. Insert another record and then delete one
echo "3. Inserting and deleting records..."
sqlite3 "$DB_PATH" "INSERT INTO contacts (name, email) VALUES ('dummy3', 'dummy3@gmail.com');"
sqlite3 "$DB_PATH" "DELETE FROM contacts WHERE name = 'dummy1';"

echo "=== Running fuselog_apply to replicate the state ==="
sudo RUST_LOG=info ./target/debug/fuselog_apply "$TARGET_DIR"


echo "=== Verifying replicated SQLite database ==="
REPLICATED_DB_PATH="$TARGET_DIR/$DB_NAME"

echo -n "Verifying database file exists... "
test -f "$REPLICATED_DB_PATH" || { echo "FAIL: Replicated database file missing!"; exit 1; }
echo "OK"

echo -n "Verifying final record count (should be 2)... "
COUNT=$(sqlite3 "$REPLICATED_DB_PATH" "SELECT COUNT(*) FROM contacts;")
test "$COUNT" = "2" || { echo "FAIL: Expected 2 records, but found $COUNT."; exit 1; }
echo "OK"

echo -n "Verifying updated record (dummy2's email)... "
DUMMY2_EMAIL=$(sqlite3 "$REPLICATED_DB_PATH" "SELECT email FROM contacts WHERE name = 'dummy2';")
test "$DUMMY2_EMAIL" = "dummy2_new@gmail.com" || { echo "FAIL: dummy2's email is incorrect. Got: '$DUMMY2_EMAIL'"; exit 1; }
echo "OK"

echo -n "Verifying deleted record (dummy1 should be gone)... "
DUMMY1_COUNT=$(sqlite3 "$REPLICATED_DB_PATH" "SELECT COUNT(*) FROM contacts WHERE name = 'dummy1';")
test "$DUMMY1_COUNT" = "0" || { echo "FAIL: dummy1 was not deleted, found $DUMMY1_COUNT records for it."; exit 1; }
echo "OK"

echo -n "Verifying remaining record (dummy3)... "
DUMMY3_COUNT=$(sqlite3 "$REPLICATED_DB_PATH" "SELECT COUNT(*) FROM contacts WHERE name = 'dummy3';")
test "$DUMMY3_COUNT" = "1" || { echo "FAIL: dummy3's record is missing."; exit 1; }
echo "OK"

echo ""
echo "=== Performing final directory comparison ==="
if diff -rq "$MOUNT_DIR" "$TARGET_DIR" >/dev/null; then
    echo "OK: Source and Mirror directories are identical."
else
    echo "FAIL: Source and Mirror directories are different."
    diff -r "$MOUNT_DIR" "$TARGET_DIR"
    exit 1
fi

echo ""
echo "All SQLite and directory comparison tests passed, yeay!"