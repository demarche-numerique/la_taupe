#!/bin/sh
# Refreshes the two reference files embedded in the binary:
#
#   src/riad_bank_name.csv        bank code -> BIC, name   (ECB list of MFIs)
#   src/code_postal_commune.csv   postal code -> commune   (La Poste, data.gouv.fr)
#
# Both are downloaded, reduced to the columns the code reads, and diffed against
# the current file before replacing it. Nothing is replaced if a download fails.
#
# Bank register: the ECB publishes a dated file (fi_mrr_csv_YYMMDD.csv.gz); the
# latest name is read from the listing page. Some institutions lose their BIC from
# one edition to the next (the ECB stops maintaining it) — the previous BIC is kept
# in that case, so the validation keeps covering them.
set -eu

cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "== bank register (ECB)"
page="https://www.ecb.europa.eu/stats/financial_corporations/list_of_financial_institutions/html/monthly_list-MID.en.html"
latest=$(curl -fsSL "$page" | grep -o 'fi_mrr_csv_[0-9]\{6\}\.csv\.gz' | sort -u | tail -1)
[ -n "$latest" ] || { echo "could not find the latest ECB file name on $page" >&2; exit 1; }
echo "   latest: $latest"
curl -fsSL "https://www.ecb.europa.eu/stats/money/mfi/general/html/dla/mfi_mrr_MID/$latest" -o "$tmp/fi.csv.gz"
gunzip -f "$tmp/fi.csv.gz"
# UTF-16, tab-separated; keep FR rows: RIAD code, BIC, name
iconv -f UTF-16 -t UTF-8 "$tmp/fi.csv" \
  | awk -F'\t' 'NR>1 && $1 ~ /^FR/ {print $1"\t"$2"\t"$4}' \
  | sort > "$tmp/riad_new.csv"
# merge: new rows win, but an emptied BIC keeps the previous value
sort src/riad_bank_name.csv > "$tmp/riad_old.csv"
awk -F'\t' -v OFS='\t' '
  NR==FNR { old_bic[$1]=$2; next }
  { if ($2=="" && ($1 in old_bic) && old_bic[$1]!="") $2=old_bic[$1]; print }
' "$tmp/riad_old.csv" "$tmp/riad_new.csv" > "$tmp/riad_merged.csv"
echo "   rows: $(wc -l < src/riad_bank_name.csv) -> $(wc -l < "$tmp/riad_merged.csv")"
echo "   with BIC: $(awk -F'\t' '$2!=""' src/riad_bank_name.csv | wc -l) -> $(awk -F'\t' '$2!=""' "$tmp/riad_merged.csv" | wc -l)"
echo "   changed lines: $(diff "$tmp/riad_old.csv" "$tmp/riad_merged.csv" | grep -c '^[<>]' || true)"
cp "$tmp/riad_merged.csv" src/riad_bank_name.csv

echo "== postal codes (La Poste)"
curl -fsSL "https://data.laposte.fr/data-fair/api/v1/datasets/laposte-hexasmal/raw" -o "$tmp/hexasmal.csv"
# Latin-1, semicolon-separated; keep postal code, delivery label
iconv -f LATIN1 -t UTF-8 "$tmp/hexasmal.csv" \
  | awk -F';' 'NR>1 {print $3";"$4}' \
  | sort -u > "$tmp/cp_new.csv"
echo "   rows: $(wc -l < src/code_postal_commune.csv) -> $(wc -l < "$tmp/cp_new.csv")"
echo "   changed lines: $(diff src/code_postal_commune.csv "$tmp/cp_new.csv" | grep -c '^[<>]' || true)"
cp "$tmp/cp_new.csv" src/code_postal_commune.csv

echo "== done — review with: git diff --stat src/riad_bank_name.csv src/code_postal_commune.csv"
