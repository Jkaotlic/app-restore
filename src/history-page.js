// Скрипт окна «История покупок Apple» (reportaproblem.apple.com). Вшивается в Rust строкой
// (`include_str!`) и ставится только этому окну. Безопасность: токены, cookie и заголовки сайта
// остаются в памяти страницы — наружу уходят только {adamId, name, type, date} приложений iPhone
// через одну команду `plugin:history|items`. Без внешних библиотек; разбор проверяет vitest.
(function (root) {
  "use strict";
  var PATH = "/api/purchase/search";
  var MAX_PAGES = 500;
  var DELAY_MS = 400;

  function isSearchUrl(url) {
    try {
      var path = new URL(String(url), "https://reportaproblem.apple.com/").pathname.replace(/\/+$/, "");
      return path.slice(-PATH.length) === PATH;
    } catch (e) {
      return false;
    }
  }

  /** Ответ `purchase/search` → приложения iPhone и следующая пачка; `null` — формат не тот. */
  function extractApps(json) {
    if (!json || typeof json !== "object" || !Array.isArray(json.purchases)) return null;
    var items = [];
    for (var i = 0; i < json.purchases.length; i++) {
      var p = json.purchases[i];
      if (!p || !Array.isArray(p.plis)) continue;
      for (var j = 0; j < p.plis.length; j++) {
        var pli = p.plis[j];
        if (!pli || pli.lineItemType !== "IOSApp" || pli.adamId === null || pli.adamId === undefined) continue;
        var lc = pli.localizedContent;
        items.push({
          adamId: String(pli.adamId),
          name: lc && typeof lc.nameForDisplay === "string" ? lc.nameForDisplay : "",
          type: "IOSApp",
          date: typeof pli.pliDate === "string" ? pli.pliDate : null,
        });
      }
    }
    var next = typeof json.nextBatchId === "string" && json.nextBatchId ? json.nextBatchId : null;
    return { items: items, next: next };
  }

  /** Тело следующего запроса: те же dsids, что у страницы. */
  function nextBody(pageBody, batchId) {
    var dsids = pageBody && Array.isArray(pageBody.dsids) ? pageBody.dsids : [];
    return { batchId: batchId, dsids: dsids };
  }

  /** Первая пачка — ответ страницы; дальше по `nextBatchId`, не больше MAX_PAGES страниц. */
  async function crawl(first, pageBody, fetchPage, send, sleep) {
    var page = extractApps(first);
    if (!page) return void (await send([], true, "api_changed"));
    await send(page.items, !page.next, null);
    var pages = 1;
    var next = page.next;
    while (next) {
      if (pages >= MAX_PAGES) return void (await send([], true, null));
      await sleep(DELAY_MS);
      var json;
      try {
        json = await fetchPage(nextBody(pageBody, next));
      } catch (e) {
        return void (await send([], true, "http"));
      }
      page = extractApps(json);
      if (!page) return void (await send([], true, "api_changed"));
      pages++;
      next = page.next;
      await send(page.items, !next, null);
    }
  }

  function install(w) {
    if (w.__appRestoreHistoryHooked) return;
    w.__appRestoreHistoryHooked = true;
    var started = false;
    var origFetch = w.fetch.bind(w);
    var send = function (items, done, error) {
      return w.__TAURI_INTERNALS__.invoke("plugin:history|items", { items: items, done: done, error: error });
    };
    var sleep = function (ms) {
      return new Promise(function (r) {
        setTimeout(r, ms);
      });
    };

    /** Первый ответ страницы (без batchId): дальше догружаем сами теми же заголовками. */
    function begin(url, method, headers, bodyText, json) {
      if (started) return;
      var pageBody = {};
      try {
        pageBody = JSON.parse(bodyText || "{}") || {};
      } catch (e) {
        pageBody = {};
      }
      if (pageBody.batchId) return;
      started = true;
      var fetchPage = function (body) {
        return origFetch(url, { method: method || "POST", headers: headers, body: JSON.stringify(body), credentials: "include" }).then(function (r) {
          if (!r.ok) throw new Error("HTTP " + r.status);
          return r.json();
        });
      };
      crawl(json, pageBody, fetchPage, send, sleep).catch(function () {
        send([], true, "http").catch(function () {});
      });
    }

    w.fetch = function (input, init) {
      var p = origFetch(input, init);
      try {
        var url = typeof input === "string" ? input : input && input.url ? input.url : String(input);
        if (isSearchUrl(url)) {
          var isReq = typeof Request !== "undefined" && input instanceof Request;
          var headers = new Headers((init && init.headers) || (isReq ? input.headers : undefined));
          var method = (init && init.method) || (isReq ? input.method : "POST");
          var bodyP = init && typeof init.body === "string" ? Promise.resolve(init.body) : isReq ? input.clone().text() : Promise.resolve("");
          Promise.all([bodyP, p.then(function (r) { return r.clone().json(); })])
            .then(function (v) { begin(url, method, headers, v[0], v[1]); })
            .catch(function () {});
        }
      } catch (e) {
        // страница работает как обычно
      }
      return p;
    };

    var X = w.XMLHttpRequest && w.XMLHttpRequest.prototype;
    if (!X) return;
    var open = X.open;
    var setHeader = X.setRequestHeader;
    var xsend = X.send;
    X.open = function (method, url) {
      this.__arh = isSearchUrl(url) ? { method: method, url: String(url), headers: new Headers() } : null;
      return open.apply(this, arguments);
    };
    X.setRequestHeader = function (k, v) {
      if (this.__arh) {
        try {
          this.__arh.headers.append(k, v);
        } catch (e) {
          // запретный заголовок — браузер подставит сам
        }
      }
      return setHeader.apply(this, arguments);
    };
    X.send = function (body) {
      var meta = this.__arh;
      if (meta) {
        var xhr = this;
        xhr.addEventListener("load", function () {
          try {
            var json = xhr.responseType === "json" ? xhr.response : JSON.parse(xhr.responseText);
            begin(meta.url, meta.method, meta.headers, typeof body === "string" ? body : "", json);
          } catch (e) {
            // не JSON — не наш ответ
          }
        });
      }
      return xsend.apply(this, arguments);
    };
  }

  root.__appRestoreHistory = { extractApps: extractApps, nextBody: nextBody, isSearchUrl: isSearchUrl, crawl: crawl };
  if (root.location && root.location.hostname === "reportaproblem.apple.com" && root.top === root && typeof root.fetch === "function") install(root);
})(globalThis);
