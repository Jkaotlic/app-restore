# Фикстуры

Сняты с живой системы 2026-10-26, ipatool 2.7.0, macOS arm64, Apple ID
с включённой 2FA (`scripts/capture-fixtures.sh`). Обезличено: email → `user@example.com`,
имя → `Test User`. Список приложений в `purchases-*` заменён на публичные приложения
(id, bundleID, name, version); форма строки и остальные поля — как у Apple.

## ipatool/ (`<сценарий>.jsonl` — stdout, `<сценарий>.exit` — код выхода)

| файл | сценарий | что важно |
|---|---|---|
| login-no-code | вход без `--auth-code` | **exit 0**, нет поля `success`, только `message` «2FA code is required…» |
| login-no-code-2 | повтор того же | то же |
| login-wrong-password | неверный пароль, без кода | **то же, что login-no-code**: Apple сначала просит код, пароль не проверен |
| login-wrong-code | код 000000 | exit 1, `error` «apple did not complete verification; try a fresh 2FA code» |
| login-ok | верный код | exit 0, `name`, `email`, `success:true` |
| login-no-password | пустой пароль (нет TTY) | exit 1, «password is required when not running in interactive mode…» |
| auth-info | после входа | exit 0, `name`, `email`, `success:true` |
| purchases-p1 / -p2 | `list-purchases --platform iphone -l 5 -p N` | `count:5`, `totalCount:741`, `page`, `apps[]{id,bundleID,name,version,price,purchaseDate,platforms}` |
| download-owned | приложение из списка покупок | exit 0, `output` = `<dir>/<bundleID>_<trackId>_<version>.ipa`, `purchased:false` |
| download-not-owned | бесплатное приложение без прав | exit 1, `error` «license is required» |
| auth-revoke | выход | exit 0, `success:true` |
| purchases-logged-out | без входа | exit 1, «failed to get account: failed to get item: The specified item could not be found in the keyring» |

Не воспроизводились вживую (429/5xx, account disabled) — строятся в тестах по той же
форме строки с текстом ошибки из исходников ipatool.

## lookup/

`https://itunes.apple.com/lookup?id=492224193,564177498,686449807&country={ru,us}&entity=software`:
Удалённое приложение из каталога (492224193) отсутствует в обоих, Telegram (686449807) есть.

## Удалённые из магазина приложения (сняты 2026-10-26 позже, тот же тестовый аккаунт)

Приложение — запись каталога (`trackId` из `catalog/ru-removed.json`), у тестового аккаунта на неё
есть права.

| файл | сценарий | что важно |
|---|---|---|
| list-versions-owned | `list-versions -i <trackId из каталога>` | exit 0, `externalVersionIdentifiers[]`, `success:true` — права есть |
| list-versions-not-owned | `list-versions -i <trackId приложения без прав>` | exit 1, «license is required» |
| download-removed-by-bundle | `download -b <bundle id из каталога>` | exit 1, «app not found»: удалённое по bundle id не находится |
| download-by-id | `download -i <trackId из каталога>` | exit 0, `output` = `<dir>/<trackId>_<version>.ipa` (без bundle id в имени) |

Для `scripts/test_catalog_check.py`, сняты curl 2026-10-26 (ответы Apple; адреса почты продавцов заменены на `support@example.com`, остальное без правок):

| файл | запрос | что важно |
|---|---|---|
| lookup-ru-removed.json | `lookup?id=492224193&country=ru&entity=software` | `resultCount:0` — приложения нет в магазине |
| search-ru-domklik.json | `search?term=Домклик&country=ru&entity=software&limit=10` | 10 результатов, ни одного с похожим названием (нет ложных кандидатов) |
| search-ru-dev-telegram.json | `search?term=Telegram FZ-LLC&attribute=softwareDeveloper&country=ru&entity=software&limit=25` | поиск по разработчику; у Telegram `sellerName` и `artistName` совпадают |
| search-ru-telegram.json | `search?term=Telegram&country=ru&entity=software&limit=5` | 5 результатов, у Telegram `sellerName` «Telegram FZ-LLC» |
| version-metadata | `get-version-metadata -i <trackId из каталога> --external-version-id <последний из list-versions>` | exit 0, `externalVersionID`, `displayVersion`, `releaseDate`, `success:true` |
