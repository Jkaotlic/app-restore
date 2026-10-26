#!/usr/bin/env python3
"""Проверка каталога catalog/ru-removed.json по публичному iTunes API (только stdlib).

Для каждой записи:
  * Lookup country=ru нашёл приложение            -> status: back_in_store;
  * иначе Lookup без страны (магазин США) и Search country=ru по имени, псевдонимам и (если
    известен seller) по разработчику -> кандидаты, которых ещё нет в каталоге:
      - тот же продавец, другой trackId: «Кандидаты: новое имя или id» (для поиска по
        разработчику ещё и совпавшее имя; иначе только счётчик «Другие приложения разработчика»);
      - продавец неизвестен или иной, но имя содержит название/псевдоним (или наоборот):
        «Неподтверждённые кандидаты (совпало имя)»;
  * нет в RU и US и Search не находит             -> счётчик «Нет в RU и US» в отчёте
    (поимённо только те, что раньше были back_in_store).
Продавец сверяется с sellerName и artistName из ответа Apple.

По умолчанию --dry-run: файл не трогается, отчёт в Markdown идёт в stdout.
Файл пишется только с --write и только если что-то изменилось.
Между запросами пауза 1 с. На 429 и 5xx — до 3 повторов с паузами 2, 5, 10 с.
"""
import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://itunes.apple.com"
PAUSE = 1
RETRY_DELAYS = (2, 5, 10)
DEFAULT_CATALOG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "catalog", "ru-removed.json")


def fetch_json(url, sleep=time.sleep, timeout=20):
    """GET -> dict. Повторы на 429/5xx и сбоях сети; остальные ошибки пробрасываются."""
    for attempt in range(len(RETRY_DELAYS) + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "app-restore-catalog-check"})
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            retryable = e.code == 429 or e.code >= 500
            if not retryable or attempt == len(RETRY_DELAYS):
                raise
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            if attempt == len(RETRY_DELAYS):
                raise
        sleep(RETRY_DELAYS[attempt])


def dumps(catalog):
    """Сериализация в формате файла каталога: поля по строкам, массивы в одну строку."""
    items = []
    for e in catalog:
        lines = [f'    "{k}": {json.dumps(v, ensure_ascii=False)}' for k, v in e.items()]
        items.append("  {\n" + ",\n".join(lines) + "\n  }")
    return "[\n" + ",\n".join(items) + "\n]\n"


class Result:
    def __init__(self, catalog, report, candidates, unconfirmed, changed):
        self.catalog = catalog
        self.report = report
        self.candidates = candidates  # тот же продавец, другой trackId
        self.unconfirmed = unconfirmed  # продавец неизвестен или иной, совпало имя
        self.changed = changed


def _norm(s):
    return (s or "").strip().casefold()


def _name_matches(track_name, terms):
    t = _norm(track_name)
    if len(t) < 3:
        return False
    return any(_norm(x) and (_norm(x) in t or t in _norm(x)) for x in terms)


def check_catalog(catalog, http_get, sleep=time.sleep, ignored=()):
    """Проверяет записи. `http_get(url) -> dict` подменяется в тестах; вход не изменяется.
    `ignored` — trackId проверенных ложных кандидатов, в отчёт они не попадают."""
    result = json.loads(json.dumps(catalog))
    known_ids = {e["track_id"] for e in result} | set(ignored)
    requests = [0]

    def get(path, **params):
        if requests[0]:
            sleep(PAUSE)
        requests[0] += 1
        return http_get(f"{API}{path}?{urllib.parse.urlencode(params)}")

    returned, gone, candidates, unconfirmed, errors = [], [], [], [], []
    gone_count = 0
    other_dev = []
    for i, e in enumerate(result):
        tid = e["track_id"]
        label = f"«{e['name']}» (id {tid})"
        was_back = e.get("status") == "back_in_store"
        try:
            ru = [r for r in get("/lookup", id=tid, country="ru", entity="software").get("results", [])
                  if r.get("trackId") == tid]
            if ru:
                e["status"] = "back_in_store"
                if not e.get("seller") and ru[0].get("sellerName"):
                    e["seller"] = ru[0]["sellerName"]
                returned.append(f"- {label}: снова в российском App Store — {ru[0].get('trackName', e['name'])}")
                continue
            if was_back:
                e["status"] = "removed"
            # без страны Lookup смотрит магазин США
            anywhere = [r for r in get("/lookup", id=tid, entity="software").get("results", [])
                        if r.get("trackId") == tid]
            if anywhere and not e.get("seller") and anywhere[0].get("sellerName"):
                e["seller"] = anywhere[0]["sellerName"]
            terms = list(dict.fromkeys([e["name"], *e.get("aliases", [])]))
            queries = [{"term": t} for t in terms]
            if e.get("seller"):
                queries.append({"term": e["seller"], "attribute": "softwareDeveloper"})
            seen_self, same, by_name, dev_only = False, {}, {}, {}
            for q in queries:
                is_dev = "attribute" in q
                for r in get("/search", country="ru", entity="software", limit=25, **q).get("results", []):
                    hid = r.get("trackId")
                    if hid == tid:
                        seen_self = True
                        continue
                    if hid in known_ids:
                        continue
                    hit = {"for_id": tid, "for_name": e["name"], "track_id": hid,
                           "name": r.get("trackName", ""), "seller": r.get("sellerName", ""), "term": q["term"]}
                    name_ok = _name_matches(r.get("trackName"), terms)
                    seller_ok = e.get("seller") and _norm(e["seller"]) in (
                        _norm(r.get("sellerName")), _norm(r.get("artistName")))
                    if is_dev and not name_ok:
                        dev_only[hid] = hit  # другое приложение разработчика: только счётчик
                    elif seller_ok:
                        same.setdefault(hid, hit)
                    elif name_ok:
                        by_name.setdefault(hid, hit)
            for hid in same:
                by_name.pop(hid, None)
            for hid in [*same, *by_name]:
                dev_only.pop(hid, None)
            if dev_only:
                other_dev.append(f"- {label}: Другие приложения разработчика: {len(dev_only)}")
            candidates.extend(same.values())
            unconfirmed.extend(by_name.values())
            if not anywhere and not seen_self and not same and not by_name:
                gone_count += 1
                if was_back:
                    gone.append(f"- {label}")
        except Exception as ex:  # сеть упала: запись остаётся как была, в отчёте — причина
            errors.append(f"- {label}: {type(ex).__name__}: {ex}")
            result[i] = json.loads(json.dumps(catalog[i]))

    def hit_line(c):
        return (f"- «{c['for_name']}» (id {c['for_id']}): найдено «{c['name']}» (id {c['track_id']}), "
                f"продавец {c['seller']}, запрос «{c['term']}»")

    lines = ["# Проверка каталога удалённых приложений", "",
             f"Записей: {len(result)}. Запросов: {requests[0]}.", ""]
    if returned:
        lines += ["## Вернулись в магазин", "", *returned, ""]
    if candidates:
        lines += ["## Кандидаты: новое имя или id", "", *map(hit_line, candidates), ""]
    if unconfirmed:
        lines += ["## Неподтверждённые кандидаты (совпало имя)", "",
                  "Продавец не подтверждён: возможно, это другое приложение с похожим названием.", "",
                  *map(hit_line, unconfirmed), ""]
    if other_dev:
        lines += [*other_dev, ""]
    if gone_count:
        lines += [f"Нет в RU и US: {gone_count} (для удалённых приложений это норма).", ""]
        if gone:
            lines += ["Раньше были в магазине, теперь снова нет:", "", *gone, ""]
    if errors:
        lines += ["## Ошибки запросов", "", *errors, ""]
    if len(lines) == 4:
        lines += ["Без изменений.", ""]
    return Result(result, "\n".join(lines), candidates, unconfirmed, result != catalog)


def load_ignored(path):
    """catalog/ignored-candidates.json: [{"track_id": N, "reason": "..."}]; нет файла — пусто."""
    try:
        with open(path, encoding="utf-8") as f:
            return {int(i["track_id"]) for i in json.load(f)}
    except FileNotFoundError:
        return set()


def main(argv=None, http_get=None, sleep=time.sleep, out=sys.stdout):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--dry-run", action="store_true", help="только отчёт (по умолчанию)")
    mode.add_argument("--write", action="store_true", help="записать изменения в каталог")
    ap.add_argument("--catalog", default=DEFAULT_CATALOG, help="путь к ru-removed.json")
    args = ap.parse_args(argv)

    with open(args.catalog, encoding="utf-8") as f:
        catalog = json.load(f)
    ignored = load_ignored(os.path.join(os.path.dirname(os.path.abspath(args.catalog)), "ignored-candidates.json"))
    res = check_catalog(catalog, http_get or (lambda u: fetch_json(u, sleep=sleep)), sleep=sleep, ignored=ignored)
    out.write(res.report)
    if args.write and res.changed:
        with open(args.catalog, "w", encoding="utf-8") as f:
            f.write(dumps(res.catalog))
    return 0


if __name__ == "__main__":
    sys.exit(main())
