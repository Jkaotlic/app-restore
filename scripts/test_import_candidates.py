"""Тесты import-candidates.py. Сеть подменяется: http_get(url) -> dict, text_get(url) -> str."""
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("import_candidates", os.path.join(HERE, "import-candidates.py"))
ic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ic)

SRC = ic.Source(repo="kda2495/IPA_Downloader", sha="c137feeb43b46e01db79d001f9d52da3c85774ed",
                path="Files/AppsList.txt")
SRC2 = ic.Source(repo="pdx15/ipatool-webGUI", sha="ab0409f246543afdb5cd0561eda3129250c57432",
                 path="resources/Apps_ID_List.txt")


def app(tid, name, seller="", artist="", genre="Finance", desc="", artist_id=None):
    return {"trackId": tid, "trackName": name, "sellerName": seller, "artistName": artist or seller,
            "primaryGenreName": genre, "description": desc, "artistId": artist_id}


class Fake:
    """lookups: (id, 'ru'|'us') -> record; cdx: url -> rows; pages: url -> html."""

    def __init__(self, lookups=None, cdx=None, pages=None):
        self.lookups, self.cdx, self.pages = lookups or {}, cdx or {}, pages or {}
        self.calls = []

    def get(self, url):
        self.calls.append(url)
        u = urlparse(url)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        if u.netloc == "itunes.apple.com":
            rec = self.lookups.get((int(q["id"]), q.get("country", "us")))
            return {"resultCount": 1 if rec else 0, "results": [rec] if rec else []}
        if u.path == "/cdx/search/cdx":
            rows = [r for k, rs in self.cdx.items() if q["url"].startswith(k) or k.startswith(q["url"])
                    for r in rs]
            return [["urlkey", "timestamp", "original", "mimetype", "statuscode"], *rows] if rows else []
        raise AssertionError(url)

    def text(self, url):
        self.calls.append(url)
        if url in self.pages:
            return self.pages[url]
        raise OSError("404")


class ParseTest(unittest.TestCase):
    def test_lines(self):
        text = ("﻿1Password 7: 568903335\r\n"
                "AppBank (MoodPetal): 6760329045\r\n"
                "Au.ru (Барахолка 24: объявления 24/7): 6760170997\r\n"
                "Rutube Studio: (RStudio): 6801144026\r\n"
                "Купер: заказать еду и продукты: 6505025679\r\n"
                "\r\n# комментарий\r\nбез id\r\n")
        items = ic.parse_list(text, SRC)
        self.assertEqual([(i.track_id, i.brand, i.alt, i.line) for i in items], [
            (568903335, "1Password 7", None, 1),
            (6760329045, "AppBank", "MoodPetal", 2),
            (6760170997, "Au.ru", "Барахолка 24: объявления 24/7", 3),
            (6801144026, "Rutube Studio", "RStudio", 4),
            (6505025679, "Купер: заказать еду и продукты", None, 5),
        ])
        self.assertEqual(items[1].source,
                         "https://github.com/kda2495/IPA_Downloader/blob/"
                         "c137feeb43b46e01db79d001f9d52da3c85774ed/Files/AppsList.txt#L2")

    def test_merge_first_source_wins_and_known_ids_dropped(self):
        a = ic.parse_list("A: 1\nB: 2\n", SRC)
        b = ic.parse_list("B2: 2\nC: 3\nD: 4\n", SRC2)
        got = ic.merge_new([a, b], known={4})
        self.assertEqual([(i.track_id, i.brand, i.source.split("/")[4]) for i in got],
                         [(1, "A", "IPA_Downloader"), (2, "B", "IPA_Downloader"), (3, "C", "ipatool-webGUI")])


class RulesTest(unittest.TestCase):
    def item(self, raw):
        return ic.parse_list(f"{raw}: 1\n", SRC)[0]

    def test_ru_relevant(self):
        self.assertTrue(ic.ru_relevant(self.item("Купер"), None)[0])  # кириллица в списке
        self.assertTrue(ic.ru_relevant(self.item("Sber (X)"), app(1, "X", "Ivan Petrov"))[0])  # бренд
        self.assertTrue(ic.ru_relevant(self.item("Foo"), app(1, "Foo", "ООО Фу"))[0])  # продавец
        self.assertTrue(ic.ru_relevant(self.item("Foo"), app(1, "Foo", "Foo LLC",
                                                             desc="Приложение для оплаты услуг"))[0])
        self.assertFalse(ic.ru_relevant(self.item("1Password 7"),
                                        app(1, "1Password 7", "AgileBits Inc.", desc="Password manager"))[0])
        # одно русское слово в длинном английском описании — не признак
        self.assertFalse(ic.ru_relevant(self.item("Adobe Scan"),
                                        app(1, "Adobe Scan", "Adobe Inc.", desc="Scan " * 50 + "привет"))[0])
        # кириллица в списке, но продавец — крупная иностранная компания
        self.assertFalse(ic.ru_relevant(self.item("Google Документы"),
                                        app(1, "Google Документы", "Google LLC", artist="Google"))[0])
        # «ao» внутри слова — не форма юрлица
        self.assertFalse(ic.ru_relevant(self.item("Kaori"), app(1, "Kaori", "Kaori Inc."))[0])
        # «vk» как часть слова — не бренд
        self.assertFalse(ic.ru_relevant(self.item("Vkontrol"), app(1, "Vkontrol", "X Inc."))[0])

    def test_name_matches(self):
        m = ic.name_matches
        self.assertTrue(m("Тинькофф", "Тинькофф: банк и деньги"))
        self.assertTrue(m("Amediateka", "Амедиатека"))  # транслит
        self.assertTrue(m("2ГИС", "2ГИС: карты и навигатор"))
        self.assertFalse(m("Сбербанк", "Calculator Pro"))
        self.assertFalse(m("VK", "VKontrol Pro"))  # бренд не целым словом

    def test_category(self):
        it = self.item
        self.assertEqual(ic.category(it("Альфа-Банк (Ромашка)"), app(1, "Ромашка", genre="Games")), "bank")
        self.assertEqual(ic.category(it("ВКонтакте"), app(1, "ВКонтакте", genre="Social Networking")), "social")
        self.assertEqual(ic.category(it("РЖД Пассажирам"), app(1, "РЖД Пассажирам", genre="Travel")), "travel")
        self.assertEqual(ic.category(it("Авито (PatrolKit)"), app(1, "PatrolKit", genre="Utilities")), "travel")
        # жанр маскировки не учитывается
        self.assertEqual(ic.category(it("Госуслуги (Fin Calc)"), app(1, "Fin Calc", genre="Finance")), "other")
        self.assertEqual(ic.category(it("Foo"), app(1, "Foo", genre="Finance")), "bank")
        self.assertEqual(ic.category(it("Foo"), None), "other")

    def test_slug(self):
        self.assertEqual(ic.slug("Чайная церемония"), "чайная-церемония")
        self.assertEqual(ic.slug("Барахолка 24: объявления 24/7"), "барахолка-24-объявления-24-7")
        self.assertEqual(ic.slug("MoodPetal"), "moodpetal")


CAT = [{"track_id": 9, "name": "Альфа-Банк", "aliases": ["Альфа"], "letter": "А", "color": "#EF3124",
        "source": "s", "category": "bank"}]


class RunTest(unittest.TestCase):
    def run_items(self, text, fake, catalog=CAT, ignored=()):
        items = ic.parse_list(text, SRC)
        return ic.run([items], catalog, fake.get, fake.text, sleep=lambda s: None, ignored=set(ignored))

    def test_lookup_ru_back_in_store_plain(self):
        f = Fake({(100, "ru"): app(100, "Купер: доставка продуктов", "ООО Купер", genre="Food & Drink",
                                   artist_id=55)})
        r = self.run_items("Купер: заказать еду и продукты: 100\n", f)
        self.assertEqual(r.added, [{
            "track_id": 100, "name": "Купер: доставка продуктов", "aliases": ["Купер: заказать еду и продукты"],
            "letter": "К", "color": r.added[0]["color"], "category": "travel",
            "source": "https://github.com/kda2495/IPA_Downloader/blob/"
                      "c137feeb43b46e01db79d001f9d52da3c85774ed/Files/AppsList.txt#L1",
            "status": "back_in_store", "seller": "ООО Купер"}])
        self.assertRegex(r.added[0]["color"], r"^#[0-9A-F]{6}$")
        self.assertEqual(r.evidence[100], "lookup ru")

    def test_disguised_us_only_reuses_brand_color(self):
        f = Fake({(101, "us"): app(101, "Ромашка", "Ivan Ivanov", genre="Games", artist_id=77)})
        r = self.run_items("Альфа-Банк (Ромашка): 101\n", f)
        e = r.added[0]
        self.assertEqual((e["name"], e["aliases"], e["status"], e["disguised"], e["artist_id"],
                          e["color"], e["letter"], e["category"]),
                         ("Ромашка", ["Альфа-Банк"], "removed", True, 77, "#EF3124", "А", "bank"))
        self.assertEqual(r.evidence[101], "lookup us")

    def test_not_ru_and_mismatch_and_known_skipped(self):
        f = Fake({(1, "us"): app(1, "1Password 7", "AgileBits Inc.", genre="Productivity", desc="Passwords"),
                  (2, "ru"): app(2, "Calculator Pro", "ООО Калк", genre="Utilities")})
        r = self.run_items("1Password 7: 1\nСбербанк: 2\nАльфа: 9\n", f, ignored={3})
        self.assertEqual(r.added, [])
        self.assertEqual([i.track_id for i, _ in r.not_ru], [1])
        self.assertEqual([i.track_id for i, _ in r.mismatch], [2])
        self.assertNotIn("id=9", " ".join(f.calls))  # уже в каталоге — без запросов

    def test_wayback_evidence(self):
        orig = "https://apps.apple.com/ru/app/%D1%87%D0%B0%D0%B9/id102"
        f = Fake(cdx={"apps.apple.com/ru/app/чай": [["k", "20260425000907", orig, "text/html", "200"]]},
                 pages={f"https://web.archive.org/web/20260425000907id_/{orig}":
                        "<html><title>Приложение «Чай» — App Store</title></html>"})
        r = self.run_items("ПСБ (Чай): 102\n", f)
        e = r.added[0]
        self.assertEqual((e["name"], e["aliases"], e["disguised"], e["confidence"], e["status"], e["category"]),
                         ("Чай", ["ПСБ"], True, "medium", "removed", "bank"))
        self.assertIn(f"https://web.archive.org/web/20260425000907/{orig}", e["source"])
        self.assertTrue(e["source"].startswith("https://github.com/kda2495/"))
        self.assertEqual(r.evidence[102], "wayback 20260425000907")

    def test_wayback_title_must_match(self):
        orig = "https://apps.apple.com/ru/app/x/id103"
        f = Fake(cdx={"apps.apple.com/ru/app/чай": [["k", "2026", orig, "text/html", "200"]]},
                 pages={f"https://web.archive.org/web/2026id_/{orig}": "<title>Другое — App Store</title>"})
        r = self.run_items("ПСБ (Чай): 103\n", f)
        self.assertEqual(r.added, [])
        self.assertEqual([i.track_id for i, _ in r.no_evidence], [103])

    def test_wayback_other_id_ignored(self):
        f = Fake(cdx={"apps.apple.com/ru/app/чай": [["k", "2026", "https://apps.apple.com/ru/app/чай/id1039",
                                                    "text/html", "200"]]},
                 pages={"https://web.archive.org/web/2026id_/https://apps.apple.com/ru/app/чай/id1039":
                        "<title>«Чай» в App Store</title>"})
        r = self.run_items("ПСБ (Чай): 103\n", f)
        self.assertEqual([i.track_id for i, _ in r.no_evidence], [103])

    def test_network_error_goes_to_errors(self):
        def boom(url):
            raise OSError("down")
        items = ic.parse_list("Сбер: 5\n", SRC)
        r = ic.run([items], CAT, boom, boom, sleep=lambda s: None)
        self.assertEqual(r.added, [])
        self.assertEqual([i.track_id for i, _ in r.errors], [5])


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


class MainTest(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.cat = os.path.join(self.d, "ru-removed.json")
        with open(self.cat, "w", encoding="utf-8") as f:
            f.write(ic.cc.dumps(CAT))
        self.lst = os.path.join(self.d, "list.txt")
        with open(self.lst, "w", encoding="utf-8") as f:
            f.write("Купер: 100\n")
        self.f = Fake({(100, "ru"): app(100, "Купер", "ООО Купер", genre="Food & Drink")})

    def tearDown(self):
        shutil.rmtree(self.d)

    def main(self, *extra):
        out = io.StringIO()
        ic.main(["--catalog", self.cat, "--ignored", os.path.join(self.d, "none.json"),
                 "--list", f"{self.lst}=kda2495/IPA_Downloader@abc:Files/AppsList.txt", *extra],
                http_get=self.f.get, text_get=self.f.text, sleep=lambda s: None, out=out)
        return out.getvalue()

    def test_dry_run_keeps_file(self):
        before = read(self.cat)
        report = self.main()
        self.assertIn("Добавлено: 1", report)
        self.assertEqual(read(self.cat), before)

    def test_write_appends_via_dumps(self):
        self.main("--write")
        text = read(self.cat)
        data = json.loads(text)
        self.assertEqual([e["track_id"] for e in data], [9, 100])
        self.assertEqual(text, ic.cc.dumps(data))
        self.assertIn("blob/abc/Files/AppsList.txt#L1", data[1]["source"])

    def test_cache_reused(self):
        cache = os.path.join(self.d, "cache.json")
        self.main("--cache", cache)
        n = len(self.f.calls)
        self.main("--cache", cache)
        self.assertEqual(len(self.f.calls), n)


if __name__ == "__main__":
    unittest.main()
