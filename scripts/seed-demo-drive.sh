#!/usr/bin/env bash
set -euo pipefail
UID_A="c3a1459f-3c3f-4ff5-bee4-8e7958bbb698"
TID_A="40894092-7ec5-4693-94f0-afb1c7fb51c4"
DRV=http://localhost:8004

CONTENT="Bem-vindo ao Expresso v4! Arquivo de demonstração."
echo -n "$CONTENT" > /tmp/welcome.txt
LEN=$(wc -c < /tmp/welcome.txt)
FNAME_B64=$(echo -n "welcome.txt" | base64 -w0)
MIME_B64=$(echo -n "text/plain" | base64 -w0)
META="filename $FNAME_B64,filetype $MIME_B64"

echo "=== TUS: create ==="
LOC=$(curl -sSi -X POST "$DRV/api/v1/drive/uploads" \
  -H "x-user-id: $UID_A" -H "x-tenant-id: $TID_A" \
  -H "Tus-Resumable: 1.0.0" -H "Upload-Length: $LEN" \
  -H "Upload-Metadata: $META" \
  | awk -F': ' 'tolower($1)=="location"{gsub(/\r/,"",$2); print $2}')
echo "Location=$LOC"
UPID=${LOC##*/}
echo "upload_id=$UPID"

echo "=== TUS: PATCH chunk ==="
curl -sSi -X PATCH "$DRV/api/v1/drive/uploads/$UPID" \
  -H "x-user-id: $UID_A" -H "x-tenant-id: $TID_A" \
  -H "Tus-Resumable: 1.0.0" -H "Upload-Offset: 0" \
  -H "Content-Type: application/offset+octet-stream" \
  --data-binary @/tmp/welcome.txt | head -15
echo
echo "=== List files ==="
curl -sS "$DRV/api/v1/drive/files" \
  -H "x-user-id: $UID_A" -H "x-tenant-id: $TID_A"
echo
