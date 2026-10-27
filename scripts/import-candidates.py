#!/usr/bin/env python3
"""Импорт кандидатов в catalog/ru-removed.json из открытых списков «Имя: trackId» (только stdlib).

Источники (MIT, коммиты закреплены в DEFAULT_LISTS):
  * kda2495/IPA_Downloader — Files/AppsList.txt;
  * pdx15/ipatool-webGUI  — resources/Apps_ID_List.txt.
Строка «Бренд (Имя в магазине): N» означает замаскированный выпуск; «Бренд: (Имя): N» — то же.

Для каждого id, которого нет в каталоге и в ignored-candidates.json:
  1. Lookup country=ru и без страны (США). Найден — это доказательство, что id есть в App Store;
     имя записи — trackName из ответа. Имя должно совпасть с брендом или именем из скобок
     (или бренд — с продавцом), иначе строка уходит в «Имя не совпало» и не добавляется.
  2. Не найден нигде — Wayback: CDX по apps.apple.com/{ru/,us/,}app/<slug имени> с тем же idN,
     снимок страницы, имя из <title> должно совпасть. Такая запись получает confidence: medium.
  3. Только российские сервисы: кириллица в названии списка или в магазине, известный российский
     бренд, российская форма продавца (ООО, АО…) или описание в магазине США по-русски.
     Остальное — в отчёт «Не про РФ».
status: back_in_store, если Lookup ru нашёл приложение, иначе removed. disguised: имя в магазине
не совпадает с брендом; тогда бренд — в aliases, аккаунт разработчика — в artist_id.
source: ссылка на строку списка на закреплённом коммите (+ снимок Wayback, если он нужен).

По умолчанию --dry-run: только отчёт в stdout. --write дописывает записи через dumps() из
catalog-check.py. --cache FILE запоминает ответы сети (повторный прогон без запросов).
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import sys
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("catalog_check", os.path.join(HERE, "catalog-check.py"))
cc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cc)

API = "https://itunes.apple.com"
CDX = "https://web.archive.org/cdx/search/cdx"
PAUSE = 1
STOREFRONTS = ("apps.apple.com/ru/app/", "apps.apple.com/us/app/", "apps.apple.com/app/")
DEFAULT_CATALOG = os.path.join(HERE, "..", "catalog", "ru-removed.json")
DEFAULT_IGNORED = os.path.join(HERE, "..", "catalog", "ignored-candidates.json")


class Source:
    def __init__(self, repo, sha, path):
        self.repo, self.sha, self.path = repo, sha, path

    def raw_url(self):
        return f"https://raw.githubusercontent.com/{self.repo}/{self.sha}/{self.path}"

    def line_url(self, n):
        return f"https://github.com/{self.repo}/blob/{self.sha}/{self.path}#L{n}"


DEFAULT_LISTS = (
    Source("kda2495/IPA_Downloader", "c137feeb43b46e01db79d001f9d52da3c85774ed", "Files/AppsList.txt"),
    Source("pdx15/ipatool-webGUI", "ab0409f246543afdb5cd0561eda3129250c57432", "resources/Apps_ID_List.txt"),
)


class Item:
    def __init__(self, track_id, raw, brand, alt, line, source):
        self.track_id, self.raw, self.brand, self.alt, self.line, self.source = \
            track_id, raw, brand, alt, line, source

    def __repr__(self):
        return f"Item({self.track_id}, {self.raw!r})"


_LINE = re.compile(r"^(.*?):\s*(\d+)\s*$")
_ALT = re.compile(r"^(.*?)\s*:?\s*\((.+)\)$")


def parse_list(text, source):
    """Строки «Имя: N». Имя «Бренд (Другое имя)» или «Бренд: (Другое имя)» — замаскированный выпуск."""
    items = []
    for n, line in enumerate(text.lstrip("﻿").splitlines(), 1):
        m = _LINE.match(line.strip())
        if not m:
            continue
        raw = m[1].strip()
        a = _ALT.match(raw)
        brand, alt = (a[1].strip().rstrip(":").strip(), a[2].strip()) if a and a[1].strip() else (raw, None)
        items.append(Item(int(m[2]), raw, brand, alt, n, source.line_url(n)))
    return items


def merge_new(lists, known):
    """Порядок первого списка; повторы и известные id отбрасываются."""
    seen, out = set(known), []
    for items in lists:
        for i in items:
            if i.track_id not in seen:
                seen.add(i.track_id)
                out.append(i)
    return out


# ---------- нормализация и сравнение имён ----------

_TR = dict(zip("абвгдеёзийклмнопрстуфхцы", "abvgdeezijklmnoprstufhcy"))
_TR.update({"ж": "zh", "ч": "ch", "ш": "sh", "щ": "sch", "ъ": "", "ь": "", "э": "e", "ю": "yu", "я": "ya"})


def norm(s):
    """Нижний регистр, транслит, только буквы-цифры и одиночные пробелы."""
    s = "".join(_TR.get(ch, ch) for ch in (s or "").casefold())
    return " ".join(re.sub(r"[^0-9a-z]+", " ", s).split())


def _heads(s):
    s = (s or "").strip()
    return {x for x in (s, re.split(r"\s*[:—–|(]\s*|\s+-\s+", s)[0]) if norm(x)}


def _contains(hay, needle):
    return bool(needle) and re.search(rf"(?<![0-9a-z]){re.escape(needle)}(?![0-9a-z])", hay) is not None


def name_matches(brand, track_name):
    """Бренд целым словом в имени магазина или наоборот (с транслитом)."""
    for b in _heads(brand):
        for t in _heads(track_name):
            nb, nt = norm(b), norm(t)
            if len(nb) < 2 or len(nt) < 2:
                continue
            if _contains(nt, nb) or (len(nt) >= 3 and _contains(nb, nt)):
                return True
            if nb.replace(" ", "") == nt.replace(" ", ""):
                return True
    return False


_CYR = re.compile(r"[а-яё]", re.I)


def _cyr_share(text):
    letters = re.findall(r"[^\W\d_]", text or "")
    return sum(1 for c in letters if _CYR.match(c)) / len(letters) if letters else 0.0


RU_BRANDS = (
    "sber", "sberbank", "tinkoff", "tbank", "t bank", "vtb", "alfa bank", "alfabank", "gazprombank", "gazprom",
    "raiffeisenbank", "sovcombank", "otkritie", "pochta bank", "rosbank", "psb", "promsvyazbank", "mkb",
    "rshb", "uralsib", "ak bars", "akbars", "mts", "megafon", "beeline", "bilajn", "tele2", "yota",
    "rostelecom", "yandex", "vk", "vkontakte", "odnoklassniki", "mail ru", "ozon", "wildberries", "avito",
    "gosuslugi", "rzd", "aeroflot", "kinopoisk", "ivi", "okko", "rutube", "wink", "kion", "2gis", "lamoda",
    "samokat", "kuper", "sbermarket", "megamarket", "magnit", "perekrestok", "pyaterochka", "vkusvill",
    "sportmaster", "zvuk", "dzen", "qiwi", "yoomoney", "mir pay", "russia", "rossii", "rossiya", "moscow",
    "moskva", "kaspersky", "amediateka", "premier", "domclick", "cian", "tutu", "aviasales",
)
_RU_FORMS = re.compile(r"(?<![0-9a-z])(ooo|oao|zao|pao|ao|pjsc)(?![0-9a-z])")


def _brand_hit(text):
    t = norm(text)
    return next((b for b in RU_BRANDS if _contains(t, b)), None)


FOREIGN_SELLERS = ("google", "google llc", "apple", "microsoft", "adobe", "meta platforms", "amazon")


def ru_relevant(item, rec):
    """(bool, причина). rec — ответ Lookup (лучше магазина США) или None."""
    if rec:
        seller = norm(rec.get("sellerName"))
        if seller in FOREIGN_SELLERS or norm(rec.get("artistName")) in FOREIGN_SELLERS:
            return False, f"иностранный продавец {rec.get('sellerName')}"
    if _CYR.search(item.raw):
        return True, "кириллица в списке"
    hit = _brand_hit(item.brand) or (item.alt and _brand_hit(item.alt))
    if hit:
        return True, f"бренд {hit}"
    if not rec:
        return False, "нет в магазине, бренд не российский"
    seller = f"{rec.get('sellerName') or ''} {rec.get('artistName') or ''}"
    if _CYR.search(seller) or _RU_FORMS.search(norm(seller)):
        return True, f"продавец {rec.get('sellerName') or rec.get('artistName')}"
    hit = _brand_hit(seller)
    if hit:
        return True, f"продавец {hit}"
    if _CYR.search(rec.get("trackName") or ""):
        return True, "кириллица в имени магазина"
    if _cyr_share(rec.get("description")) > 0.5:
        return True, "описание по-русски"
    return False, f"продавец {rec.get('sellerName') or '?'}"


# ---------- категория, буква, цвет ----------

_KW = (
    ("bank", ("bank", "banka", "banki", "sber", "sberbank", "tinkoff", "tbank", "vtb", "alfa", "gazprombank",
              "raiffeisen", "raiffeisenbank", "sovcombank", "otkritie", "rosbank", "psb", "mkb", "rshb",
              "uralsib", "ak bars", "invest", "investicii", "investitsii", "broker", "koshelek", "wallet",
              "dengi", "money", "finance", "qiwi", "yoomoney", "mir pay", "pay")),
    ("social", ("vk", "vkontakte", "odnoklassniki", "ok", "mail", "mail ru", "telegram", "max", "rutube",
                "kinopoisk", "ivi", "okko", "premier", "wink", "kion", "start", "amediateka", "more tv",
                "dzen", "yappy", "tenchat", "video", "tv", "kino")),
    ("travel", ("rzd", "aeroflot", "pobeda", "s7", "utair", "tutu", "aviasales", "taksi", "taxi", "metro",
                "transport", "2gis", "karty", "avito", "ozon", "wildberries", "wb", "lamoda", "market",
                "lavka", "eda", "samokat", "kuper", "sbermarket", "megamarket", "dostavka", "delivery",
                "magnit", "perekrestok", "lenta", "pyaterochka", "vkusvill", "sportmaster", "mvideo",
                "eldorado", "citilink", "cian", "domclick", "avto ru", "drom", "yula", "barakholka",
                "ostrovok", "sutochno", "au ru", "go")),
)
_GENRES = {"Finance": "bank", "Social Networking": "social", "Photo & Video": "social",
           "Entertainment": "social", "Travel": "travel", "Navigation": "travel", "Shopping": "travel",
           "Food & Drink": "travel", "Lifestyle": "travel"}


def category(item, rec):
    """По ключевым словам бренда; у незамаскированных — ещё имя и жанр магазина."""
    disguised = item.alt is not None or (rec and not name_matches(item.brand, rec.get("trackName")))
    texts = [item.brand] + ([] if disguised or not rec else [rec.get("trackName") or ""])
    for cat, words in _KW:
        if any(_contains(norm(t), w) for t in texts for w in words):
            return cat
    if rec and not disguised:
        return _GENRES.get(rec.get("primaryGenreName"), "other")
    return "other"


PALETTE = ("#E53935", "#D81B60", "#8E24AA", "#5E35B1", "#3949AB", "#1E88E5", "#039BE5", "#00897B",
           "#43A047", "#7CB342", "#F4511E", "#6D4C41", "#546E7A", "#FB8C00")


def _letter(brand):
    ch = next((c for c in brand if c.isalnum()), "?")
    return ch.upper()


def _color(brand, catalog):
    keys = {norm(h) for h in _heads(brand)}
    for e in catalog:
        if keys & {norm(x) for x in [e.get("name", ""), *e.get("aliases", [])]}:
            return e["color"].upper()
    return PALETTE[int(hashlib.md5(norm(brand).encode()).hexdigest(), 16) % len(PALETTE)]


def slug(name):
    """Как Apple строит путь страницы: нижний регистр, всё кроме букв и цифр — дефис."""
    return "-".join(re.findall(r"[^\W_]+", (name or "").casefold()))


_TITLE = re.compile(r"<title[^>]*>(.*?)</title>", re.I | re.S)


def _title_name(html_text):
    m = _TITLE.search(html_text or "")
    if not m:
        return None
    t = cc.html.unescape(m[1]).replace("‎", "").strip()
    t = re.sub(r"^App\s*Store\s*:\s*", "", t, flags=re.I)
    q = re.search(r"«(.+?)»", t)
    if q:
        return q[1].strip()
    t = re.sub(r"\s*(?:[—–-]\s*)?(?:в|on the|on|in the)?\s*App\s*Store.*$", "", t, flags=re.I).strip()
    return t or None


# ---------- прогон ----------

class Result:
    def __init__(self):
        self.added, self.evidence = [], {}
        self.not_ru, self.mismatch, self.no_evidence, self.errors = [], [], [], []
        self.report = ""


def run(lists, catalog, http_get, text_get, sleep=time.sleep, ignored=(), log=lambda msg: None):
    """lists — результаты parse_list. Вход не меняется; новые записи — в Result.added.
    log(msg) — ход прогона (в main — stderr)."""
    known = {e["track_id"] for e in catalog} | set(ignored)
    items = merge_new(lists, known)
    pool = list(catalog)
    res = Result()
    calls = [0]

    def net(fn, url):
        if calls[0]:
            sleep(PAUSE)
        calls[0] += 1
        return fn(url)

    def lookup(tid, country):
        params = {"id": tid, "entity": "software", **({"country": country} if country else {})}
        data = net(http_get, f"{API}/lookup?{urllib.parse.urlencode(params)}")
        return next((r for r in data.get("results", []) if r.get("trackId") == tid), None)

    def wayback(item):
        names = list(dict.fromkeys(n for n in (item.alt, *_heads(item.brand)) if n and slug(n)))
        idre = re.compile(rf"/id{item.track_id}(?!\d)")
        for n in names:
            for sf in STOREFRONTS:
                q = urllib.parse.urlencode({"url": sf + slug(n), "matchType": "prefix", "output": "json",
                                            "filter": "statuscode:200", "limit": 500})
                rows = net(http_get, f"{CDX}?{q}") or []
                hits = [r for r in rows[1:] if idre.search(r[2])]
                for r in hits[-3:]:
                    ts, orig = r[1], r[2]
                    try:
                        page = net(text_get, f"https://web.archive.org/web/{ts}id_/{orig}")
                    except Exception:
                        continue
                    title = _title_name(page)
                    if title and any(name_matches(x, title) for x in (item.alt, item.brand) if x):
                        return title, ts, f"https://web.archive.org/web/{ts}/{orig}"
        return None

    for n, item in enumerate(items, 1):
        if n % 25 == 0:
            log(f"{n}/{len(items)}: добавлено {len(res.added)}, запросов {calls[0]}")
        try:
            ru = lookup(item.track_id, "ru")
            us = lookup(item.track_id, None)
            rec = ru or us
            ok, why = ru_relevant(item, us or ru)
            if not ok:
                res.not_ru.append((item, why))
                continue
            if rec:
                name = rec.get("trackName") or ""
                seller = f"{rec.get('sellerName') or ''} | {rec.get('artistName') or ''}"
                if not (name_matches(item.brand, name) or (item.alt and name_matches(item.alt, name))
                        or name_matches(item.brand, rec.get("sellerName")) or
                        name_matches(item.brand, rec.get("artistName"))):
                    res.mismatch.append((item, f"в магазине «{name}», продавец {seller}"))
                    continue
                source, evidence, conf = item.source, "lookup ru" if ru else "lookup us", None
            else:
                wb = wayback(item)
                if not wb:
                    res.no_evidence.append((item, "нет в RU и US, нет снимка Wayback с этим именем"))
                    continue
                name, ts, wb_url = wb
                source, evidence, conf = f"{item.source} {wb_url}", f"wayback {ts}", "medium"
        except Exception as ex:
            res.errors.append((item, f"{type(ex).__name__}: {ex}"))
            continue
        disguised = not name_matches(item.brand, name)
        aliases = [x for x in dict.fromkeys([item.brand] + ([item.alt] if item.alt and disguised else []))
                   if norm(x) != norm(name)]
        e = {"track_id": item.track_id, "name": name, "aliases": aliases, "letter": _letter(item.brand),
             "color": _color(item.brand, pool), "category": category(item, rec), "source": source,
             "status": "back_in_store" if ru else "removed"}
        if rec and rec.get("sellerName"):
            e["seller"] = rec["sellerName"]
        if disguised:
            e["disguised"] = True
            if rec and rec.get("artistId"):
                e["artist_id"] = rec["artistId"]
        if conf:
            e["confidence"] = conf
        res.added.append(e)
        res.evidence[item.track_id] = evidence
        pool.append(e)
    res.report = _report(res, len(items), calls[0])
    return res


def _report(res, total, calls):
    def row(i, why):
        return f"- {i.track_id} «{i.raw}» — {why} ({i.source})"
    lines = ["# Импорт кандидатов в каталог", "",
             f"Новых id в списках: {total}. Запросов: {calls}.", f"Добавлено: {len(res.added)}.", ""]
    if res.added:
        lines += ["## Добавлено", "", "| id | имя в магазине | бренд | категория | статус | доказательство |",
                  "|---|---|---|---|---|---|"]
        for e in res.added:
            brand = ", ".join(e["aliases"]) or "—"
            mark = " (замаскировано)" if e.get("disguised") else ""
            lines.append(f"| {e['track_id']} | {e['name']}{mark} | {brand} | {e['category']} | {e['status']} "
                         f"| {res.evidence[e['track_id']]} |")
        lines.append("")
    for title, rows in (("Имя не совпало (не добавлены)", res.mismatch),
                        ("Нет доказательства (не добавлены)", res.no_evidence),
                        ("Ошибки запросов", res.errors), ("Не про РФ", res.not_ru)):
        if rows:
            lines += [f"## {title}: {len(rows)}", "", *(row(i, w) for i, w in rows), ""]
    return "\n".join(lines)


# ---------- запуск ----------

def _cached(fn, cache, save, pace):
    """Ответ из кэша — без паузы; пауза только перед настоящим запросом."""
    def get(url):
        if url not in cache:
            pace()
            cache[url] = fn(url)
            save()
        return cache[url]
    return get


def _parse_list_arg(s):
    """FILE=owner/repo@sha:path"""
    path, _, spec = s.partition("=")
    repo, _, rest = spec.partition("@")
    sha, _, sub = rest.partition(":")
    return path, Source(repo, sha, sub)


def main(argv=None, http_get=None, text_get=None, sleep=time.sleep, out=sys.stdout):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--catalog", default=DEFAULT_CATALOG)
    p.add_argument("--ignored", default=DEFAULT_IGNORED)
    p.add_argument("--list", action="append", default=[],
                   help="локальный файл списка: FILE=owner/repo@sha:path (без него — DEFAULT_LISTS с GitHub)")
    p.add_argument("--cache", help="JSON-кэш ответов сети")
    g = p.add_mutually_exclusive_group()
    g.add_argument("--dry-run", action="store_true", default=True)
    g.add_argument("--write", action="store_true")
    a = p.parse_args(argv)
    http_get = http_get or cc.fetch_json
    text_get = text_get or cc.fetch_text
    cache = {}
    if a.cache and os.path.exists(a.cache):
        with open(a.cache, encoding="utf-8") as f:
            cache = json.load(f)
    fresh = [0]

    def save(force=False):
        fresh[0] += 1
        if a.cache and (force or fresh[0] % 25 == 0):
            with open(a.cache + ".tmp", "w", encoding="utf-8") as f:
                json.dump(cache, f, ensure_ascii=False)
            os.replace(a.cache + ".tmp", a.cache)

    started = [False]

    def pace():
        if started[0]:
            sleep(PAUSE)
        started[0] = True

    hget, tget = _cached(http_get, cache, save, pace), _cached(text_get, cache, save, pace)

    with open(a.catalog, encoding="utf-8") as f:
        catalog = json.load(f)
    if a.list:
        lists = []
        for s in a.list:
            path, src = _parse_list_arg(s)
            with open(path, encoding="utf-8-sig") as f:
                lists.append(parse_list(f.read(), src))
    else:
        lists = [parse_list(tget(src.raw_url()), src) for src in DEFAULT_LISTS]
    try:
        res = run(lists, catalog, hget, tget, sleep=lambda _: None, ignored=cc.load_ignored(a.ignored),
                  log=lambda m: print(m, file=sys.stderr, flush=True))
    finally:
        save(force=True)
    out.write(res.report + "\n")
    if a.write and res.added:
        with open(a.catalog, "w", encoding="utf-8") as f:
            f.write(cc.dumps(catalog + res.added))
    return 0


if __name__ == "__main__":
    sys.exit(main())
