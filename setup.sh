#!/bin/bash

MOUNT_DIR="/tmp/fuse_my/"
sudo rm -rf "$MOUNT_DIR"
sudo pkill -f fuselog_core || true
fusermount -u "$MOUNT_DIR" 2>/dev/null || true
sudo rm -rf "$MOUNT_DIR"
sudo rm -f "/tmp/fuselog.sock"
sudo rm -f /var/cache/fuselog/statediff.dict
sudo rm -rf *.bin
docker compose down -v --remove-orphans || true