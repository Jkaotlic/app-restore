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

Отдельно ищутся замаскированные выпуски (раздел «Замаскированные выпуски — кандидаты»):
  * статьи лент из catalog/news-sources.json за 14 дней: название или псевдоним записи в заголовке
    или описании, в статье есть ссылка apps.apple.com/.../idNNN; id берётся только из ссылки;
  * записи `disguised: true` с `seller`: другие приложения того же аккаунта разработчика.
Каждый кандидат проверяется Lookup в ru и без страны (США). В каталог кандидаты не пишутся никогда;
сбой одного источника попадает в «Ошибки источников новостей» и прогон не роняет.
`--only-news` — только этот раздел (каталог не проверяется и не пишется).

По умолчанию --dry-run: файл не трогается, отчёт в Markdown идёт в stdout.
Файл пишется только с --write и только если что-то изменилось.
Между запросами пауза 1 с. На 429 и 5xx — до 3 повторов с паузами 2, 5, 10 с.
"""
import argparse
import email.utils
import http.client
import html
import json
import os
import re
import sys
import time
import datetime
import urllib.error
import urllib.parse
import urllib.request

API = "https://itunes.apple.com"
PAUSE = 1
USER_AGENT = "app-restore-catalog-check (+https://github.com/Jkaotlic/app-restore)"
NEWS_DAYS = 14
MAX_ARTICLES = 25
RETRY_DELAYS = (2, 5, 10)
DEFAULT_CATALOG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "catalog", "ru-removed.json")


def _fetch(url, sleep, timeout, limit=None):
    """GET -> bytes. Повторы на 429/5xx и сбоях сети; остальные ошибки пробрасываются."""
    for attempt in range(len(RETRY_DELAYS) + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read(limit) if limit else resp.read()
        except http.client.IncompleteRead as e:
            return e.partial  # сервер оборвал ленту: берём, что пришло (разбор терпит обрыв)
        except urllib.error.HTTPError as e:
            retryable = e.code == 429 or e.code >= 500
            if not retryable or attempt == len(RETRY_DELAYS):
                raise
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            if attempt == len(RETRY_DELAYS):
                raise
        sleep(RETRY_DELAYS[attempt])


def fetch_json(url, sleep=time.sleep, timeout=20):
    """GET -> dict."""
    return json.loads(_fetch(url, sleep, timeout).decode("utf-8"))


def fetch_text(url, sleep=time.sleep, timeout=20):
    """GET -> str (лента или статья); не больше 3 МБ."""
    return _fetch(url, sleep, timeout, limit=3_000_000).decode("utf-8", errors="replace")


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


# ---------- Замаскированные выпуски: новости и аккаунты продавцов ----------

APPLE_LINK = re.compile(r"apps\.apple\.com/[^\s\"'<>]*?/id(\d+)", re.I)
APPLE_LINK_NOCC = re.compile(r"apps\.apple\.com/id(\d+)", re.I)


def apple_ids(text):
    """trackId только из ссылок apps.apple.com/.../idNNN, без повторов, в порядке появления."""
    found = sorted([(m.start(), int(m.group(1))) for m in APPLE_LINK.finditer(text)] +
                   [(m.start(), int(m.group(1))) for m in APPLE_LINK_NOCC.finditer(text)])
    return list(dict.fromkeys(i for _, i in found))


def _tag(block, name):
    m = re.search(rf"<(?:\w+:)?{name}\b[^>]*>(.*?)</(?:\w+:)?{name}>", block, re.S | re.I)
    if not m:
        return ""
    v = m.group(1).strip()
    c = re.fullmatch(r"<!\[CDATA\[(.*?)\]\]>", v, re.S)
    return c.group(1) if c else html.unescape(v)


def _plain(t):
    return html.unescape(re.sub(r"<[^>]+>", " ", t))


def parse_feed(text):
    """RSS -> [{title, link, date (aware datetime | None), text, raw}]. Регулярками: ленты бывают битыми."""
    items = []
    for m in re.finditer(r"<item\b.*?</item>", text, re.S | re.I):
        block = m.group(0)
        date = None
        try:
            date = email.utils.parsedate_to_datetime(_tag(block, "pubDate"))
            if date.tzinfo is None:
                date = date.replace(tzinfo=datetime.timezone.utc)
        except (TypeError, ValueError):
            pass
        title = _tag(block, "title")
        body = _tag(block, "description") + " " + _tag(block, "encoded")
        items.append({"title": _plain(title).strip(), "link": _tag(block, "link").strip(), "date": date,
                      "text": _plain(title + " " + body), "raw": block})
    return items


class NewsResult:
    def __init__(self, candidates, errors, report, requests_api):
        self.candidates = candidates
        self.errors = errors
        self.report = report
        self.requests_api = requests_api


def _brand_patterns(catalog):
    pats = {}
    for e in catalog:
        for t in dict.fromkeys([e["name"], *e.get("aliases", [])]):
            if len(_norm(t)) >= 3 and _norm(t) not in pats:
                pats[_norm(t)] = (t, re.compile(r"(?<!\w)" + re.escape(_norm(t)) + r"(?!\w)"))
    return pats


def find_news_candidates(catalog, sources, http_get, text_get, sleep=time.sleep, ignored=(), now=None,
                         days=NEWS_DAYS, max_articles=MAX_ARTICLES):
    """Кандидаты в замаскированные выпуски; каталог не меняется. `text_get(url) -> str` — лента/статья."""
    now = now or datetime.datetime.now(datetime.timezone.utc)
    known = {e["track_id"] for e in catalog} | set(ignored)
    cands, errors = {}, []
    counters = {"all": 0, "api": 0, "articles": 0, "fetched": 0}

    def pause():
        if counters["all"]:
            sleep(PAUSE)
        counters["all"] += 1

    def api(path, **params):
        pause()
        counters["api"] += 1
        return http_get(f"{API}{path}?{urllib.parse.urlencode(params)}")

    def add(tid, **kw):
        c = cands.get(tid)
        if c is None:
            cands[tid] = {"track_id": tid, "via": kw.pop("via"), "title": "", "store": None, "store_name": "",
                          "seller": "", "brands": [], "source": "", "for_name": "", **kw}
            return
        for k, v in kw.items():
            if k == "brands":
                c["brands"] = list(dict.fromkeys([*c["brands"], *v]))
            elif k != "via" and v and not c.get(k):
                c[k] = v

    # --- источник 1: ленты новостей
    pats = _brand_patterns(catalog)
    cutoff = now - datetime.timedelta(days=days)
    seen_articles, articles_read = set(), 0
    for src in sources:
        try:
            pause()
            items = parse_feed(text_get(src["url"]))
        except Exception as ex:
            errors.append(f"- {src.get('name', src['url'])}: лента {src['url']}: {type(ex).__name__}: {ex}")
            continue
        for it in items:
            if it["date"] and it["date"] < cutoff:
                continue
            articles_read += 1
            low = _norm(it["text"])
            brands = [orig for k, (orig, rx) in pats.items() if rx.search(low)]
            if not brands or not it["link"] or it["link"] in seen_articles:
                continue
            seen_articles.add(it["link"])
            ids = apple_ids(it["raw"])
            if counters["articles"] < max_articles:
                counters["articles"] += 1
                try:
                    pause()
                    ids = list(dict.fromkeys([*ids, *apple_ids(text_get(it["link"]))]))
                except Exception as ex:
                    errors.append(f"- {src.get('name', src['url'])}: статья {it['link']}: {type(ex).__name__}: {ex}")
            for tid in ids:
                if tid not in known:
                    add(tid, via="news", title=it["title"], brands=brands, source=it["link"])

    # состояние в магазине: ru, затем без страны (США)
    for tid, c in list(cands.items()):
        try:
            ru = [r for r in api("/lookup", id=tid, country="ru", entity="software").get("results", [])
                  if r.get("trackId") == tid]
            us = [] if ru else [r for r in api("/lookup", id=tid, entity="software").get("results", [])
                                if r.get("trackId") == tid]
        except Exception as ex:
            c["store"] = "error"
            errors.append(f"- id {tid}: проверка в магазине: {type(ex).__name__}: {ex}")
            continue
        hit = (ru or us or [None])[0]
        if hit:
            c["store"] = "ru" if ru else "us"
            c["store_name"] = hit.get("trackName", "")
            c["seller"] = c["seller"] or hit.get("sellerName", "")

    # --- источник 2: другие приложения аккаунтов продавцов замаскированных выпусков
    sellers = {}
    for e in catalog:
        if e.get("disguised") and e.get("seller"):
            sellers.setdefault(e["seller"], []).append(e)
    for seller, entries in sellers.items():
        try:
            artist = next((e["artist_id"] for e in entries if e.get("artist_id")), None)
            for e in entries:
                if artist:
                    break
                for country in ("ru", None):
                    params = {"id": e["track_id"], "entity": "software"}
                    if country:
                        params["country"] = country
                    r = [x for x in api("/lookup", **params).get("results", []) if x.get("trackId") == e["track_id"]]
                    if r and r[0].get("artistId"):
                        artist = r[0]["artistId"]
                        break
            if not artist:
                for r in api("/search", country="ru", entity="software", limit=25, term=seller,
                             attribute="softwareDeveloper").get("results", []):
                    if seller in (r.get("artistName"), r.get("sellerName")) and r.get("artistId"):
                        artist = r["artistId"]
                        break
            if not artist:
                continue
            for r in api("/lookup", id=artist, country="ru", entity="software", limit=200).get("results", []):
                tid = r.get("trackId")
                if r.get("wrapperType") == "artist" or not tid or tid in known:
                    continue
                add(tid, via="seller", store="ru", store_name=r.get("trackName", ""), seller=seller,
                    source="аккаунт разработчика", for_name=entries[0]["name"])
        except Exception as ex:
            errors.append(f"- продавец «{seller}»: {type(ex).__name__}: {ex}")

    # --- отчёт
    def state(c):
        return {"ru": "есть в российском App Store", "us": "только вне РФ (магазин США)",
                "error": "проверка магазина не удалась"}.get(c["store"], "нет в магазине")

    def line(c):
        parts = [f"- id {c['track_id']} «{c['store_name'] or c['title'] or '?'}» — {state(c)}"]
        if c["brands"]:
            parts.append("бренд: " + ", ".join(c["brands"]))
        if c["seller"]:
            parts.append("продавец: " + c["seller"] + (f" (запись «{c['for_name']}»)" if c["for_name"] else ""))
        parts.append("источник: " + c["source"])
        return "; ".join(parts)

    lines = []
    if cands:
        lines += ["## Замаскированные выпуски — кандидаты", "",
                  "В каталог сами не попадают: проверьте и добавьте вручную (или в `ignored-candidates.json`).", "",
                  *map(line, cands.values()), ""]
    if errors:
        lines += ["## Ошибки источников новостей", "", *errors, ""]
    if sources:
        lines += [f"Новости за {days} дн.: источников {len(sources)}, статей {articles_read}, "
                  f"прочитано страниц {counters['articles']}.", ""]
    return NewsResult(list(cands.values()), errors, "\n".join(lines), counters["api"])


def load_json_list(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        return []


def load_ignored(path):
    """catalog/ignored-candidates.json: [{"track_id": N, "reason": "..."}]; нет файла — пусто."""
    try:
        with open(path, encoding="utf-8") as f:
            return {int(i["track_id"]) for i in json.load(f)}
    except FileNotFoundError:
        return set()


def main(argv=None, http_get=None, sleep=time.sleep, out=sys.stdout, text_get=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--dry-run", action="store_true", help="только отчёт (по умолчанию)")
    mode.add_argument("--write", action="store_true", help="записать изменения в каталог")
    ap.add_argument("--only-news", action="store_true",
                    help="только раздел «Замаскированные выпуски» (каталог не проверяется и не пишется)")
    ap.add_argument("--catalog", default=DEFAULT_CATALOG, help="путь к ru-removed.json")
    ap.add_argument("--news-sources", help="путь к news-sources.json (по умолчанию рядом с каталогом)")
    args = ap.parse_args(argv)

    with open(args.catalog, encoding="utf-8") as f:
        catalog = json.load(f)
    cat_dir = os.path.dirname(os.path.abspath(args.catalog))
    ignored = load_ignored(os.path.join(cat_dir, "ignored-candidates.json"))
    http_get = http_get or (lambda u: fetch_json(u, sleep=sleep))
    text_get = text_get or (lambda u: fetch_text(u, sleep=sleep))
    res = None
    if not args.only_news:
        res = check_catalog(catalog, http_get, sleep=sleep, ignored=ignored)
        out.write(res.report)
    sources = load_json_list(args.news_sources or os.path.join(cat_dir, "news-sources.json"))
    news = find_news_candidates(catalog, sources, http_get, text_get, sleep=sleep, ignored=ignored)
    if news.report:
        out.write(("\n" if res else "# Замаскированные выпуски\n\n") + news.report)
    # кандидаты в файл не попадают: пишется только res.catalog (статусы и продавцы)
    if res and args.write and res.changed:
        with open(args.catalog, "w", encoding="utf-8") as f:
            f.write(dumps(res.catalog))
    return 0


if __name__ == "__main__":
    sys.exit(main())
