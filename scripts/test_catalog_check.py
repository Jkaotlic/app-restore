"""Тесты catalog-check.py. Ответы Apple подменяются записанными JSON из
src-tauri/tests/fixtures/lookup/; сеть и паузы не используются."""
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
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

    def test_real_catalog_roundtrips_byte_for_byte(self):
        real = os.path.join(HERE, "..", "catalog", "ru-removed.json")
        with open(real, encoding="utf-8") as f:
            text = f.read()
        self.assertEqual(cc.dumps(json.loads(text)), text)


if __name__ == "__main__":
    unittest.main()
