"""Тесты catalog-check.py. Ответы Apple подменяются записанными JSON из
src-tauri/tests/fixtures/lookup/; сеть и паузы не используются."""
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
from datetime import datetime, timezone
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
FIX = os.path.join(HERE, "..", "src-tauri", "tests", "fixtures", "lookup")
spec = importlib.util.spec_from_file_location("catalog_check", os.path.join(HERE, "catalog-check.py"))
cc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cc)


def fixture(name):
    with open(os.path.join(FIX, name), encoding="utf-8") as f:
        return json.load(f)


EMPTY = {"resultCount": 0, "results": []}


class FakeApple:
    """Роутер по URL: lookup (id, country) и search (term, country)."""

    def __init__(self, lookups=None, searches=None):
        self.lookups = lookups or {}  # (id, country|None) -> dict
        self.searches = searches or {}  # term -> dict
        self.calls = []

    def __call__(self, url):
        self.calls.append(url)
        u = urlparse(url)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        if u.path == "/lookup":
            return self.lookups.get((int(q["id"]), q.get("country")), EMPTY)
        assert u.path == "/search", url
        assert q["country"] == "ru" and q["entity"] == "software", url
        return self.searches.get(q["term"], EMPTY)


def entry(track_id, name, **kw):
    e = {"track_id": track_id, "name": name, "aliases": [], "letter": "Т", "color": "#112233", "source": "s"}
    e.update(kw)
    return e


def run(catalog, apple):
    sleeps = []
    res = cc.check_catalog(catalog, apple, sleep=sleeps.append)
    return res, sleeps


class CheckTests(unittest.TestCase):
    def test_returned_app_becomes_back_in_store(self):
        # Telegram (686449807) реально есть в ru Lookup
        apple = FakeApple({(686449807, "ru"): fixture("lookup-ru.json")})
        res, _ = run([entry(686449807, "Telegram")], apple)
        self.assertEqual(res.catalog[0]["status"], "back_in_store")
        self.assertIn("686449807", res.report)
        self.assertIn("Вернулись в магазин", res.report)
        self.assertTrue(res.changed)

    def test_same_seller_other_id_is_candidate(self):
        apple = FakeApple(searches={"Telegram": fixture("search-ru-telegram.json")})
        e = entry(999, "Telegram", seller="Telegram FZ-LLC")
        res, _ = run([e], apple)
        self.assertEqual(len(res.candidates), 1)
        self.assertEqual(res.candidates[0]["track_id"], 686449807)
        self.assertIn("Кандидаты: новое имя или id", res.report)
        self.assertIn("686449807", res.report)
        self.assertNotIn("id_dead?", res.report)
        self.assertFalse(res.changed)  # каталог сам по себе не меняется

    def test_name_match_without_seller_is_unconfirmed_candidate(self):
        apple = FakeApple(searches={"Telegram": fixture("search-ru-telegram.json")})
        for e in (entry(999, "Telegram"), entry(999, "Telegram", seller="Someone Else")):
            res, _ = run([e], apple)
            self.assertEqual(res.candidates, [])
            self.assertEqual([c["track_id"] for c in res.unconfirmed], [686449807])
            self.assertIn("Неподтверждённые кандидаты (совпало имя)", res.report)
            self.assertNotIn("## Кандидаты", res.report)

    def test_name_match_works_in_reverse_and_by_alias(self):
        apple = FakeApple(searches={"Telegram Messenger Plus": fixture("search-ru-telegram.json"),
                                    "Tg": fixture("search-ru-telegram.json")})
        res, _ = run([entry(999, "Telegram Messenger Plus")], apple)  # trackName внутри имени
        self.assertEqual([c["track_id"] for c in res.unconfirmed], [686449807])
        res, _ = run([entry(999, "Tg", aliases=["Telegram Messenger"])], FakeApple(
            searches={"Telegram Messenger": fixture("search-ru-telegram.json")}))
        self.assertEqual([c["track_id"] for c in res.unconfirmed], [686449807])

    def test_unrelated_hits_are_not_candidates(self):
        # реальный ответ на «Домклик»: ни одного приложения с таким именем
        apple = FakeApple(searches={"Домклик": fixture("search-ru-domklik.json"),
                                    "ДКлик": fixture("search-ru-domklik.json")})
        res, _ = run([entry(1660762523, "ДКлик", aliases=["Домклик"])], apple)
        self.assertEqual((res.candidates, res.unconfirmed), ([], []))

    def test_developer_search_runs_when_seller_known(self):
        apple = FakeApple(searches={"Telegram FZ-LLC": fixture("search-ru-dev-telegram.json")})
        run([entry(999, "Совсем другое имя", seller="Telegram FZ-LLC")], apple)
        dev = [u for u in apple.calls if "softwareDeveloper" in u]
        self.assertEqual(len(dev), 1)
        self.assertIn("term=Telegram+FZ-LLC", dev[0])
        no_seller = FakeApple()
        run([entry(999, "X")], no_seller)
        self.assertFalse([u for u in no_seller.calls if "softwareDeveloper" in u])

    def test_developer_only_hits_are_a_count_not_candidates(self):
        apple = FakeApple(searches={"Telegram FZ-LLC": fixture("search-ru-dev-telegram.json")})
        res, _ = run([entry(999, "Совсем другое имя", seller="Telegram FZ-LLC")], apple)
        self.assertEqual((res.candidates, res.unconfirmed), ([], []))
        n = len(fixture("search-ru-dev-telegram.json")["results"])
        self.assertIn(f"Другие приложения разработчика: {n}", res.report)
        self.assertNotIn("686449807", res.report)
        self.assertNotIn("## Кандидаты", res.report)
        self.assertNotIn("Неподтверждённые", res.report)

    def test_developer_hit_that_also_matches_name_is_candidate(self):
        apple = FakeApple(searches={"Telegram FZ-LLC": fixture("search-ru-dev-telegram.json")})
        res, _ = run([entry(999, "Telegram", seller="Telegram FZ-LLC")], apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [686449807])
        # прочие приложения с «Telegram» в названии, но от других продавцов — неподтверждённые
        self.assertNotIn(686449807, [c["track_id"] for c in res.unconfirmed])
        self.assertIn("Другие приложения разработчика", res.report)

    def test_artist_name_also_confirms_seller(self):
        fx = fixture("search-ru-telegram.json")
        hit = dict(fx["results"][0], sellerName="Other Legal Entity", artistName="Telegram FZ-LLC")
        apple = FakeApple(searches={"Zzz": {"resultCount": 1, "results": [hit]}})
        res, _ = run([entry(999, "Zzz", seller="Telegram FZ-LLC")], apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [686449807])

    def test_ignored_ids_never_appear(self):
        apple = FakeApple(searches={"Telegram": fixture("search-ru-telegram.json")})
        res = cc.check_catalog([entry(999, "Telegram", seller="Telegram FZ-LLC")], apple,
                               sleep=lambda s: None, ignored={686449807})
        self.assertEqual((res.candidates, res.unconfirmed), ([], []))
        self.assertNotIn("686449807", res.report)
        self.assertNotIn("## Кандидаты", res.report)
        res = cc.check_catalog([entry(999, "Telegram")], apple, sleep=lambda s: None, ignored={686449807})
        self.assertEqual(res.unconfirmed, [])

    def test_ids_already_in_catalog_are_excluded(self):
        apple = FakeApple(searches={"Telegram": fixture("search-ru-telegram.json")})
        res, _ = run([entry(999, "Telegram", seller="Telegram FZ-LLC"), entry(686449807, "Telegram Messenger")], apple)
        self.assertEqual((res.candidates, res.unconfirmed), ([], []))

    def test_aliases_are_searched_too(self):
        apple = FakeApple(searches={"Телега": fixture("search-ru-telegram.json")})
        e = entry(999, "Что-то", aliases=["Телега"], seller="Telegram FZ-LLC")
        res, _ = run([e], apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [686449807])

    def test_not_in_ru_and_us_is_only_a_count(self):
        res, _ = run([entry(492224193, "СберБанк Онлайн")], FakeApple())
        self.assertIn("Нет в RU и US: 1", res.report)
        self.assertNotIn("492224193", res.report)
        self.assertNotIn("id_dead?", res.report)
        self.assertEqual(res.catalog[0].get("status"), None)
        self.assertFalse(res.changed)

    def test_previously_back_in_store_is_listed_when_gone(self):
        res, _ = run([entry(5, "X", status="back_in_store")], FakeApple())
        self.assertIn("Нет в RU и US: 1", res.report)
        self.assertIn("(id 5)", res.report)

    def test_exists_outside_ru_is_not_dead_and_seller_is_recorded(self):
        apple = FakeApple({(686449807, None): fixture("lookup-ru.json")})
        res, _ = run([entry(686449807, "Telegram")], apple)
        self.assertNotIn("Нет в RU и US", res.report)
        self.assertEqual(res.catalog[0]["seller"], "Telegram FZ-LLC")

    def test_recorded_removed_fixture_means_not_found(self):
        apple = FakeApple({(492224193, "ru"): fixture("lookup-ru-removed.json")})
        res, _ = run([entry(492224193, "СберБанк Онлайн")], apple)
        self.assertNotIn("status", res.catalog[0])

    def test_back_in_store_reverts_when_gone_again(self):
        res, _ = run([entry(5, "X", status="back_in_store")], FakeApple())
        self.assertEqual(res.catalog[0]["status"], "removed")

    def test_one_second_pause_between_requests_not_before_first(self):
        res, sleeps = run([entry(1, "A"), entry(2, "B")], FakeApple())
        # у каждой записи: lookup ru, lookup без страны, поиск по имени
        self.assertEqual(len(sleeps), 6 - 1)
        self.assertTrue(all(s == 1 for s in sleeps))

    def test_http_error_is_reported_and_entry_untouched(self):
        def boom(url):
            raise OSError("down")
        res, _ = run([entry(1, "A")], boom)
        self.assertIn("Ошибки запросов", res.report)
        self.assertNotIn("Нет в RU и US", res.report)
        self.assertEqual(res.catalog, [entry(1, "A")])

    def test_input_not_mutated_and_rerun_is_idempotent(self):
        apple = FakeApple({(686449807, "ru"): fixture("lookup-ru.json")})
        cat = [entry(686449807, "Telegram")]
        before = json.dumps(cat)
        r1, _ = run(cat, apple)
        self.assertEqual(json.dumps(cat), before)
        r2, _ = run(r1.catalog, apple)
        self.assertEqual(r2.catalog, r1.catalog)
        self.assertFalse(r2.changed)
        self.assertEqual(r2.report, r1.report)


class FileTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        self.path = os.path.join(self.dir, "ru-removed.json")
        self.apple = FakeApple({(686449807, "ru"): fixture("lookup-ru.json")})
        self.original = cc.dumps([entry(686449807, "Telegram", aliases=["ТГ"])])
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(self.original)

    def main(self, *argv):
        out = io.StringIO()
        code = cc.main(["--catalog", self.path, *argv], http_get=self.apple, sleep=lambda s: None, out=out)
        return code, out.getvalue()

    def read(self):
        with open(self.path, encoding="utf-8") as f:
            return f.read()

    def test_dry_run_is_default_and_does_not_touch_file(self):
        code, out = self.main()
        self.assertEqual(code, 0)
        self.assertIn("Вернулись в магазин", out)
        self.assertEqual(self.read(), self.original)
        self.main("--dry-run")
        self.assertEqual(self.read(), self.original)

    def test_write_updates_file_then_second_write_is_noop(self):
        self.main("--write")
        written = self.read()
        self.assertNotEqual(written, self.original)
        self.assertEqual(json.loads(written)[0]["status"], "back_in_store")
        self.assertEqual(json.loads(written)[0]["aliases"], ["ТГ"])
        self.main("--write")
        self.assertEqual(self.read(), written)
        os.utime(self.path, (1, 1))
        self.main("--write")
        self.assertEqual(os.stat(self.path).st_mtime, 1)  # без изменений файл не переписывается

    def test_ignore_file_next_to_catalog_is_read(self):
        self.apple = FakeApple(searches={"Telegram": fixture("search-ru-telegram.json")})
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(cc.dumps([entry(999, "Telegram")]))
        _, out = self.main()
        self.assertIn("Неподтверждённые", out)
        with open(os.path.join(self.dir, "ignored-candidates.json"), "w", encoding="utf-8") as f:
            json.dump([{"track_id": 686449807, "reason": "другое приложение"}], f)
        _, out = self.main()
        self.assertNotIn("Неподтверждённые", out)

    def test_real_ignore_file_is_valid(self):
        with open(os.path.join(HERE, "..", "catalog", "ignored-candidates.json"), encoding="utf-8") as f:
            items = json.load(f)
        self.assertIn(1069871095, [i["track_id"] for i in items])
        self.assertTrue(all(i["reason"] for i in items))

    def test_write_and_dry_run_together_is_error(self):
        with self.assertRaises(SystemExit):
            self.main("--write", "--dry-run")

    def test_real_catalog_has_confirmed_sellers_and_no_owned_field(self):
        real = os.path.join(HERE, "..", "catalog", "ru-removed.json")
        with open(real, encoding="utf-8") as f:
            cat = json.load(f)
        by_id = {e["track_id"]: e for e in cat}
        self.assertGreaterEqual(sum(1 for e in cat if e.get("seller")), 10)
        self.assertEqual(by_id[1143031400]["seller"], "Центр недвижимости от Сбербанка")
        self.assertEqual(by_id[564177498]["seller"], "V Kontakte OOO")
        self.assertTrue(all("owned" not in e for e in cat))
        self.assertTrue(by_id[492224193]["source"].startswith("https://web.archive.org/"))

    def test_real_catalog_entries_have_a_known_category(self):
        real = os.path.join(HERE, "..", "catalog", "ru-removed.json")
        with open(real, encoding="utf-8") as f:
            cat = json.load(f)
        bad = [e["track_id"] for e in cat if e.get("category") not in ("bank", "social", "travel", "other")]
        self.assertEqual(bad, [])

    def test_real_catalog_roundtrips_byte_for_byte(self):
        real = os.path.join(HERE, "..", "catalog", "ru-removed.json")
        with open(real, encoding="utf-8") as f:
            text = f.read()
        self.assertEqual(cc.dumps(json.loads(text)), text)

NOW = datetime(2026, 10, 27, 12, 0, tzinfo=timezone.utc)
TG = 686449807  # реальный trackId из записанных ответов Apple


def rss(*items):
    body = "".join(
        f"<item><title><![CDATA[{t}]]></title><link>{l}</link><pubDate>{d}</pubDate>"
        f"<description><![CDATA[{desc}]]></description></item>" for t, l, d, desc in items)
    return f'<?xml version="1.0"?><rss version="2.0"><channel><title>x</title>{body}</channel></rss>'


FRESH = "Mon, 26 Oct 2026 18:47:28 +0000"
OLD = "Mon, 05 Oct 2026 18:47:28 +0000"
FEED = "https://news.example.com/feed"
ART = "https://news.example.com/a/1"
SOURCES = [{"name": "Example", "url": FEED}]


def article(*ids, extra=""):
    links = "".join(f'<a href="https://apps.apple.com/ru/app/x/id{i}?mt=8">ссылка</a>' for i in ids)
    return f"<html><body><p>Текст</p>{links}{extra}</body></html>"


class FakeWeb:
    def __init__(self, pages):
        self.pages = pages
        self.calls = []

    def __call__(self, url):
        self.calls.append(url)
        if url not in self.pages:
            raise OSError("404")
        v = self.pages[url]
        if isinstance(v, Exception):
            raise v
        return v


def news(catalog, pages, apple=None, sources=SOURCES, **kw):
    sleeps = []
    web = FakeWeb(pages)
    res = cc.find_news_candidates(catalog, sources, apple or FakeApple(), web, sleep=sleeps.append, now=NOW, **kw)
    return res, web, sleeps


SBOL = entry(1631810724, "СБОЛ", aliases=["СберБанк Онлайн"], disguised=True)


class NewsTests(unittest.TestCase):
    def test_parse_feed_tolerates_broken_xml_and_cdata(self):
        text = rss(("A &amp; B", "https://x/1", FRESH, "<p>описание</p>")) + "<item><title>обрыв"
        items = cc.parse_feed(text)
        self.assertEqual(items[0]["title"], "A & B")
        self.assertEqual(items[0]["link"], "https://x/1")
        self.assertIn("описание", items[0]["text"])
        self.assertEqual(items[0]["date"], datetime(2026, 10, 26, 18, 47, 28, tzinfo=timezone.utc))

    def test_apple_ids_only_from_links(self):
        t = ('<a href="https://apps.apple.com/ru/app/foo/id123456?mt=8">a</a> id999999 '
             'apps.apple.com/us/app/id777 https://example.com/id555555 '
             '<a href="https://apps.apple.com/app/id123456">dup</a>')
        self.assertEqual(cc.apple_ids(t), [123456, 777])

    def test_article_with_brand_and_link_gives_candidate(self):
        feed = rss(("СберБанк Онлайн вернулся", ART, FRESH, "кратко"))
        apple = FakeApple({(TG, "ru"): fixture("lookup-ru.json")})
        res, web, _ = news([SBOL], {FEED: feed, ART: article(TG, 1631810724)}, apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [TG])  # известный id не кандидат
        c = res.candidates[0]
        self.assertEqual((c["store"], c["source"]), ("ru", ART))
        self.assertIn("СберБанк Онлайн", c["brands"])
        self.assertIn("## Замаскированные выпуски — кандидаты", res.report)
        self.assertIn(f"id {TG}", res.report)
        self.assertIn("Telegram Messenger", res.report)
        self.assertIn(ART, res.report)
        self.assertIn("СберБанк Онлайн", res.report)

    def test_old_and_brandless_articles_are_not_fetched(self):
        feed = rss(("СБОЛ вернулся", "https://news.example.com/old", OLD, ""),
                   ("Про погоду", "https://news.example.com/other", FRESH, "ничего"))
        res, web, _ = news([SBOL], {FEED: feed})
        self.assertEqual(res.candidates, [])
        self.assertEqual(web.calls, [FEED])
        self.assertNotIn("## Замаскированные", res.report)

    def test_brand_in_description_counts_and_short_words_need_word_boundary(self):
        short = entry(5, "ВК", disguised=True)
        feed = rss(("Новость", ART, FRESH, "вернулся сбербанк онлайн"),
                   ("Ивкр", "https://news.example.com/b", FRESH, "ничего"))
        res, web, _ = news([SBOL, short], {FEED: feed, ART: article(TG)}, FakeApple({(TG, "ru"): fixture("lookup-ru.json")}))
        self.assertEqual([c["track_id"] for c in res.candidates], [TG])
        self.assertNotIn("https://news.example.com/b", web.calls)

    def test_known_and_ignored_ids_excluded(self):
        feed = rss(("СБОЛ", ART, FRESH, ""))
        pages = {FEED: feed, ART: article(TG)}
        res, _, _ = news([SBOL, entry(TG, "Telegram")], pages)
        self.assertEqual(res.candidates, [])
        res, _, _ = news([SBOL], pages, ignored={TG})
        self.assertEqual(res.candidates, [])

    def test_us_only_and_not_in_store_states(self):
        feed = rss(("СБОЛ", ART, FRESH, ""))
        apple = FakeApple({(TG, None): fixture("lookup-us.json")})
        res, _, _ = news([SBOL], {FEED: feed, ART: article(TG, 4242)}, apple)
        by = {c["track_id"]: c for c in res.candidates}
        self.assertEqual(by[TG]["store"], "us")
        self.assertEqual(by[4242]["store"], None)
        self.assertEqual(by[4242]["store_name"], "")
        self.assertIn("СБОЛ", by[4242]["title"])  # имя из статьи
        self.assertIn("нет в магазине", res.report)
        self.assertIn("только вне РФ", res.report)

    def test_feed_failure_does_not_stop_others_and_is_reported(self):
        feed2 = "https://other.example.com/feed"
        srcs = [{"name": "Bad", "url": FEED}, {"name": "Good", "url": feed2}]
        pages = {feed2: rss(("СБОЛ", ART, FRESH, "")), ART: article(TG)}
        res, _, _ = news([SBOL], pages, FakeApple({(TG, "ru"): fixture("lookup-ru.json")}), sources=srcs)
        self.assertEqual([c["track_id"] for c in res.candidates], [TG])
        self.assertTrue(any("Bad" in e for e in res.errors))
        self.assertIn("## Ошибки источников новостей", res.report)

    def test_article_and_lookup_failures_are_reported(self):
        feed = rss(("СБОЛ", ART, FRESH, ""), ("СБОЛ 2", "https://news.example.com/2", FRESH, ""))
        res, _, _ = news([SBOL], {FEED: feed, ART: OSError("boom"), "https://news.example.com/2": article(TG)},
                         FakeApple())
        self.assertTrue(any(ART in e for e in res.errors))
        self.assertEqual([c["track_id"] for c in res.candidates], [TG])

        def broken(url):
            raise OSError("down")
        web = FakeWeb({FEED: feed, ART: article(TG), "https://news.example.com/2": article(TG)})
        res = cc.find_news_candidates([SBOL], SOURCES, broken, web, sleep=lambda s: None, now=NOW)
        self.assertTrue(res.errors)  # lookup упал: ошибка в отчёте, прогон жив

    def test_article_fetch_cap(self):
        items = [(f"СБОЛ {i}", f"https://news.example.com/{i}", FRESH, "") for i in range(10)]
        pages = {FEED: rss(*items)}
        res, web, _ = news([SBOL], pages, max_articles=3)
        self.assertEqual(len(web.calls), 1 + 3)

    def test_pause_between_every_request_not_before_first(self):
        feed = rss(("СБОЛ", ART, FRESH, ""))
        res, web, sleeps = news([SBOL], {FEED: feed, ART: article(TG)}, FakeApple({(TG, "ru"): fixture("lookup-ru.json")}))
        total = len(web.calls) + res.requests_api
        self.assertEqual(len(sleeps), total - 1)
        self.assertTrue(all(s == 1 for s in sleeps))

    def test_seller_new_apps_of_same_account_are_candidates(self):
        e = entry(1631810724, "СБОЛ", disguised=True, seller="LLC Open IT-Solutions")
        art = {"wrapperType": "artist", "artistId": 77}
        sw = lambda tid, name: {"wrapperType": "software", "trackId": tid, "trackName": name,
                                "artistId": 77, "sellerName": "LLC Open IT-Solutions"}
        apple = FakeApple({
            (1631810724, "ru"): EMPTY,
            (1631810724, None): {"results": [{"wrapperType": "software", "trackId": 1631810724, "artistId": 77}]},
            (77, "ru"): {"results": [art, sw(1631810724, "СБОЛ"), sw(888, "Новое имя")]}})
        res, web, _ = news([e], {FEED: rss()}, apple)
        self.assertEqual([(c["track_id"], c["via"], c["store"]) for c in res.candidates], [(888, "seller", "ru")])
        self.assertEqual(res.candidates[0]["store_name"], "Новое имя")
        self.assertIn("LLC Open IT-Solutions", res.report)
        self.assertIn("id 888", res.report)

    def test_seller_saved_artist_id_skips_trackid_lookup_and_non_disguised_ignored(self):
        e = entry(10, "A", disguised=True, seller="S", artist_id=77)
        plain = entry(11, "B", seller="Other")  # не замаскированный: не смотрим
        apple = FakeApple({(77, "ru"): {"results": [{"wrapperType": "artist"},
                                                    {"wrapperType": "software", "trackId": 5, "trackName": "N"}]}})
        res, _, _ = news([e, plain], {FEED: rss()}, apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [5])
        self.assertEqual([u for u in apple.calls if "id=10" in u], [])
        self.assertFalse([u for u in apple.calls if "Other" in u or "id=11" in u])

    def test_seller_artist_found_by_developer_search_when_app_gone(self):
        e = entry(10, "A", disguised=True, seller="Dev Co")
        hit = {"wrapperType": "software", "trackId": 6, "trackName": "Z", "artistId": 99,
               "artistName": "Dev Co", "sellerName": "Dev Co"}
        apple = FakeApple(lookups={(99, "ru"): {"results": [{"wrapperType": "artist"}, hit]}},
                          searches={"Dev Co": {"results": [hit]}})
        res, _, _ = news([e], {FEED: rss()}, apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [6])

    def test_candidates_dedup_between_sources(self):
        e = entry(10, "СБОЛ", disguised=True, seller="S", artist_id=77)
        apple = FakeApple({(77, "ru"): {"results": [{"wrapperType": "software", "trackId": TG, "trackName": "T"}]},
                           (TG, "ru"): fixture("lookup-ru.json")})
        res, _, _ = news([e], {FEED: rss(("СБОЛ", ART, FRESH, "")), ART: article(TG)}, apple)
        self.assertEqual([c["track_id"] for c in res.candidates], [TG])

    def test_no_sources_no_section(self):
        res, web, _ = news([SBOL], {}, sources=[])
        self.assertEqual((res.candidates, res.errors), ([], []))
        self.assertNotIn("## ", res.report)


class NewsMainTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        self.path = os.path.join(self.dir, "ru-removed.json")
        self.apple = FakeApple({(686449807, "ru"): fixture("lookup-ru.json")})
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(cc.dumps([entry(686449807, "Telegram", aliases=["ТГ"])]))
        with open(os.path.join(self.dir, "news-sources.json"), "w", encoding="utf-8") as f:
            json.dump(SOURCES, f)
        self.web = FakeWeb({FEED: rss(("Telegram", ART, FRESH, "")), ART: article(5555)})

    def read(self):
        with open(self.path, encoding="utf-8") as f:
            return f.read()

    def run_main(self, *argv):
        out = io.StringIO()
        cc.main(["--catalog", self.path, *argv], http_get=self.apple, text_get=self.web,
                sleep=lambda s: None, out=out)
        return out.getvalue()

    def test_candidates_never_written_to_catalog(self):
        self.apple = FakeApple({(686449807, "ru"): fixture("lookup-ru.json")})
        out = self.run_main("--write")
        self.assertIn("Замаскированные выпуски", out)
        self.assertIn("id 5555", out)
        saved = json.loads(self.read())
        self.assertEqual([e["track_id"] for e in saved], [686449807])
        self.assertNotIn("5555", self.read())

    def test_only_news_makes_no_catalog_lookups_and_never_writes(self):
        d = entry(686449807, "Telegram", disguised=True)
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(cc.dumps([d]))
        before = self.read()
        out = self.run_main("--only-news", "--write")
        self.assertIn("id 5555", out)
        self.assertNotIn("Проверка каталога удалённых приложений", out)
        self.assertEqual(self.read(), before)
        self.assertFalse([u for u in self.apple.calls if "/search" in u])

    def test_missing_sources_file_is_fine(self):
        os.remove(os.path.join(self.dir, "news-sources.json"))
        out = self.run_main()
        self.assertNotIn("Замаскированные", out)

    def test_real_news_sources_file_and_workflow(self):
        with open(os.path.join(HERE, "..", "catalog", "news-sources.json"), encoding="utf-8") as f:
            srcs = json.load(f)
        self.assertGreaterEqual(len(srcs), 5)
        self.assertTrue(all(s["name"] and s["url"].startswith("https://") for s in srcs))
        with open(os.path.join(HERE, "..", ".github", "workflows", "catalog-check.yml"), encoding="utf-8") as f:
            self.assertIn("Замаскированные выпуски", f.read())


if __name__ == "__main__":
    unittest.main()
