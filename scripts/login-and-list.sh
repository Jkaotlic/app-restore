#!/usr/bin/env bash
# Вход в Apple ID и выгрузка всех покупок для iPhone в <out_dir>/purchases-all.jsonl.
# Пароль и код вводятся с клавиатуры, никуда не сохраняются.
# Использование: login-and-list.sh <ipatool> <out_dir> <apple_id>
set -u
IPA="$1"; OUT="$2"; EMAIL="$3"
[ -t 0 ] || { echo "Нужен ввод с клавиатуры: запустите в окне Терминала, не через «!»"; exit 2; }
mkdir -p "$OUT"
G=(--non-interactive --format json)

read -rsp "Пароль Apple ID (не сохраняется): " PW; echo
"$IPA" auth login -e "$EMAIL" -p "$PW" "${G[@]}" | tail -1
read -rp "Код с iPhone: " CODE
"$IPA" auth login -e "$EMAIL" -p "$PW" --auth-code "$CODE" "${G[@]}" | grep -q '"success":true' \
  && echo "Вход выполнен" || { echo "Вход не удался"; unset PW CODE; exit 1; }
unset PW CODE

: >"$OUT/purchases-all.jsonl"
p=1
while :; do
  line=$("$IPA" list-purchases --platform iphone -l 100 -p "$p" "${G[@]}" | tail -1)
  echo "$line" >>"$OUT/purchases-all.jsonl"
  n=$(printf '%s' "$line" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin).get("apps") or []))')
  echo "страница $p: $n"
  [ "$n" -eq 0 ] && break
  p=$((p+1)); [ "$p" -gt 20 ] && break
done
echo "Готово: $OUT/purchases-all.jsonl"
