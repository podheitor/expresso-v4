#!/usr/bin/env bash
# Seed demo data for Expresso v4 — alice@expresso.local
set -euo pipefail
UID_A="c3a1459f-3c3f-4ff5-bee4-8e7958bbb698"
TID_A="40894092-7ec5-4693-94f0-afb1c7fb51c4"
H=(-H "x-user-id: $UID_A" -H "x-tenant-id: $TID_A")

CAL=http://localhost:8002
CON=http://localhost:8003
DRV=http://localhost:8004
MAI=http://localhost:8001

echo "=== Calendar: create 'Pessoal' + 2 events ==="
CAL_ID=$(curl -sS "${H[@]}" -H 'content-type: application/json' \
  -X POST "$CAL/api/v1/calendars" \
  -d '{"name":"Pessoal","description":"Agenda pessoal","color":"#2563eb"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
echo "cal_id=$CAL_ID"
DT1=$(date -u -d 'tomorrow 10:00' +%Y%m%dT%H%M%SZ)
DT2=$(date -u -d 'tomorrow 11:00' +%Y%m%dT%H%M%SZ)
DT3=$(date -u -d '+3 days 14:00' +%Y%m%dT%H%M%SZ)
DT4=$(date -u -d '+3 days 15:30' +%Y%m%dT%H%M%SZ)
UID1=$(uuidgen); UID2=$(uuidgen)
cat > /tmp/ev1.ics << ICS
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Expresso//Demo//EN
BEGIN:VEVENT
UID:$UID1
DTSTAMP:$DT1
DTSTART:$DT1
DTEND:$DT2
SUMMARY:Daily Stand-up
DESCRIPTION:Sincronização diária do time
LOCATION:Meet: /meet/daily
END:VEVENT
END:VCALENDAR
ICS
cat > /tmp/ev2.ics << ICS
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Expresso//Demo//EN
BEGIN:VEVENT
UID:$UID2
DTSTAMP:$DT3
DTSTART:$DT3
DTEND:$DT4
SUMMARY:Demo Expresso v4
DESCRIPTION:Apresentação pros stakeholders
LOCATION:Sala Virtual
END:VEVENT
END:VCALENDAR
ICS
curl -sS "${H[@]}" -H 'content-type: text/calendar' -X POST \
  "$CAL/api/v1/calendars/$CAL_ID/events" --data-binary @/tmp/ev1.ics -o /dev/null -w 'ev1:%{http_code}\n'
curl -sS "${H[@]}" -H 'content-type: text/calendar' -X POST \
  "$CAL/api/v1/calendars/$CAL_ID/events" --data-binary @/tmp/ev2.ics -o /dev/null -w 'ev2:%{http_code}\n'

echo "=== Contacts: create 'Pessoal' + 5 contacts ==="
AB_ID=$(curl -sS "${H[@]}" -H 'content-type: application/json' \
  -X POST "$CON/api/v1/addressbooks" \
  -d '{"name":"Pessoal","description":"Contatos principais"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
echo "ab_id=$AB_ID"
for i in 1 2 3 4 5; do
  VUID=$(uuidgen)
  case $i in
    1) NAME="Maria Silva"; EM="maria@expresso.local"; TEL="+55 11 98800-0001";;
    2) NAME="João Santos"; EM="joao@expresso.local"; TEL="+55 11 98800-0002";;
    3) NAME="Ana Costa";   EM="ana@expresso.local";  TEL="+55 11 98800-0003";;
    4) NAME="Pedro Lima";  EM="pedro@expresso.local";TEL="+55 11 98800-0004";;
    5) NAME="Sofia Rocha"; EM="sofia@expresso.local";TEL="+55 11 98800-0005";;
  esac
  cat > /tmp/c.vcf << VCF
BEGIN:VCARD
VERSION:3.0
UID:$VUID
FN:$NAME
N:${NAME##* };${NAME% *};;;
EMAIL:$EM
TEL:$TEL
END:VCARD
VCF
  curl -sS "${H[@]}" -H 'content-type: text/vcard' -X POST \
    "$CON/api/v1/addressbooks/$AB_ID/contacts" --data-binary @/tmp/c.vcf \
    -o /dev/null -w "contact-$i:%{http_code}\n"
done

echo "=== Drive: upload 1 file ==="
echo "Bem-vindo ao Expresso v4! Este é um arquivo de demonstração." > /tmp/welcome.txt
curl -sS "${H[@]}" -F "file=@/tmp/welcome.txt" \
  "$DRV/api/v1/drive/uploads" -o /tmp/up.json -w 'upload:%{http_code}\n'
cat /tmp/up.json; echo

echo "=== Mail: create folder + message ==="
# Try creating a folder
curl -sS "${H[@]}" -H 'content-type: application/json' -X POST \
  "$MAI/mail/folders" -d '{"name":"INBOX"}' -o /tmp/fld.json -w 'folder:%{http_code}\n' || true
cat /tmp/fld.json; echo

echo "=== DONE ==="
