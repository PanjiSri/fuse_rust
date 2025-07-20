#!/bin/bash 

MOUNT_DIR="/tmp/fuse_my/"
mkdir -p "$MOUNT_DIR"
sudo ADAPTIVE_DEV_MODE=true FUSELOG_COMPRESSION=true FUSELOG_PRUNE=false WRITE_COALESCING=false ADAPTIVE_COMPRESSION=true RUST_LOG=info ./target/release/fuselog_core "$MOUNT_DIR"