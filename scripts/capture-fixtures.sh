#!/usr/bin/env bash
# Снимает живые образцы вывода ipatool для тестов.
# Пароль и код вводятся с клавиатуры, никуда не сохраняются.
# Купленное приложение берётся первым из списка покупок, некупленное —
# первое из кандидатов, на котором Apple ответит «license is required».
# Использование: capture-fixtures.sh <ipatool> <out_dir> <apple_id>
set -u
IPA="$1"; OUT="$2"; EMAIL="$3"
NOT_OWNED_CANDIDATES=(gov.nasa.NASA com.ookla.speedtest org.wikimedia.wikipedia)
[ -t 0 ] || { echo "Нужен ввод с клавиатуры: запустите в окне Терминала, не через «!»"; exit 2; }
mkdir -p "$OUT/dl"
G=(--non-interactive --format json)

run() { # run <name> <args...>
  local name="$1"; shift
  "$IPA" "$@" "${G[@]}" >"$OUT/$name.stdout" 2>"$OUT/$name.stderr"
  echo $? >"$OUT/$name.exit"
  echo "== $name: exit $(cat "$OUT/$name.exit")"
}

read -rsp "Пароль Apple ID (не сохраняется): " PW; echo
run login-wrong-password auth login -e "$EMAIL" -p "${PW}x"
run login-no-code        auth login -e "$EMAIL" -p "$PW"
read -rp "Введите НЕВЕРНЫЙ код, например 000000: " BAD
run login-wrong-code     auth login -e "$EMAIL" -p "$PW" --auth-code "$BAD"
run login-no-code-2      auth login -e "$EMAIL" -p "$PW"
read -rp "Новый код с iPhone (верный): " CODE
run login-ok             auth login -e "$EMAIL" -p "$PW" --auth-code "$CODE"
unset PW CODE BAD
run auth-info            auth info
run purchases-p1         list-purchases --platform iphone -l 5 -p 1
run purchases-p2         list-purchases --platform iphone -l 5 -p 2

OWNED=$(cat "$OUT/purchases-p1.stdout" "$OUT/purchases-p1.stderr" | python3 -I -c '
import json, sys
for line in sys.stdin:
    try: d = json.loads(line)
    except ValueError: continue
    for a in d.get("apps") or []:
        if a.get("bundleID"): print(a["bundleID"]); sys.exit()')
if [ -n "$OWNED" ]; then
  echo "Купленное для образца: $OWNED"
  run download-owned download -b "$OWNED" -o "$OUT/dl"
else
  echo "!! список покупок пуст или не разобран — download-owned пропущен"
fi

for b in "${NOT_OWNED_CANDIDATES[@]}"; do
  run download-not-owned download -b "$b" -o "$OUT/dl"
  if grep -qi "license is required" "$OUT/download-not-owned.stdout" "$OUT/download-not-owned.stderr"; then
    echo "Некупленное для образца: $b"; break
  fi
done

run auth-revoke          auth revoke
run purchases-logged-out list-purchases --platform iphone -l 5 -p 1
ls -la "$OUT/dl"
echo "Готово: $OUT"
