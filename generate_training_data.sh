#!/bin/bash

set -e

API_URL="http://localhost:8000/api/books"
GET_DIFF_TOOL="./target/release/get_diff"


create_book() {
  local title="$1"
  local author="$2"
  
  echo "-> Creating book: '$title'"
  
  curl -s -o /dev/null -X POST \
    -H "Content-Type: application/json" \
    -d "{\"title\": \"$title\", \"author\": \"$author\"}" \
    "$API_URL"
}


echo "---"
echo "STEP 1: Generating first statediff"
echo "---"

create_book "First Book" "Author A"

echo "  -> Collecting baseline diff into 'diff_baseline.bin'"
sudo $GET_DIFF_TOOL > diff_baseline.bin

echo
echo "---"
echo "STEP 2: Generating 5 samples to trigger dictionary training"
echo "---"

for i in {1..20}
do
  echo "Sample #$i of 20:"
  
  create_book "Training Book #$i" "Author B"
  
  echo "  -> Header of generated diff:"
  sudo $GET_DIFF_TOOL | hexdump -C | head -n 1
  echo 

done


echo "DONE"