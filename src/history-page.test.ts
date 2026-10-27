import { describe, expect, it } from "vitest";
import script from "./history-page.js?raw";
import fixture from "../src-tauri/tests/fixtures/rap/purchase-search.json";

type Item = { adamId: string; name: string; type: string; date: string | null };
type Page = { items: Item[]; next: string | null } | null;
interface Api {
  extractApps(json: unknown): Page;
  nextBody(pageBody: unknown, batchId: string): { batchId: string; dsids: unknown[] };
  isSearchUrl(url: string): boolean;
  crawl(
    first: unknown,
    pageBody: unknown,
    fetchPage: (body: unknown) => Promise<unknown>,
    send: (items: Item[], done: boolean, error: string | null) => Promise<unknown>,
    sleep: (ms: number) => Promise<void>,
  ): Promise<void>;
}

/** Скрипт окна — обычный скрипт страницы: исполняем его на подставном `globalThis` без `location`
 * (крючки fetch/XHR ставятся только на reportaproblem.apple.com). */
function load(): Api {
  const root: Record<string, unknown> = {};
  new Function("globalThis", script)(root);
  return root.__appRestoreHistory as Api;
}

const page = (ids: string[], next: string | null) => ({
  nextBatchId: next,
  purchases: [{ plis: ids.map((id) => ({ adamId: id, lineItemType: "IOSApp", pliDate: "2026-01-01T00:00:00Z", localizedContent: { nameForDisplay: `App ${id}` } })) }],
});

describe("history page script", () => {
  it("does not hook anything outside reportaproblem", () => {
    const root: Record<string, unknown> = { location: { hostname: "example.com" }, fetch: () => undefined };
    const f = root.fetch;
    new Function("globalThis", script)(root);
    expect(root.fetch).toBe(f);
  });

  it("extracts only iPhone apps from the synthetic fixture", () => {
    const api = load();
    expect(api.extractApps(fixture)).toEqual({
      items: [{ adamId: "6000000001", name: "Пример: музыка", type: "IOSApp", date: "2026-04-09T11:34:34.565Z" }],
      next: "Ch0Isynthetic",
    });
  });

  it("treats a response without purchases as a changed API", () => {
    const api = load();
    expect(api.extractApps({ error: "x" })).toBeNull();
    expect(api.extractApps(null)).toBeNull();
    expect(api.extractApps({ purchases: [], nextBatchId: "" })).toEqual({ items: [], next: null });
  });

  it("tolerates odd plis", () => {
    const api = load();
    const json = { purchases: [null, { plis: [null, { lineItemType: "IOSApp" }, { lineItemType: "IOSApp", adamId: 1234567, localizedContent: null }] }] };
    expect(api.extractApps(json)!.items).toEqual([{ adamId: "1234567", name: "", type: "IOSApp", date: null }]);
  });

  it("builds the next request from the page's own dsids", () => {
    const api = load();
    expect(api.nextBody({ batchId: null, dsids: ["1000000001"] }, "B2")).toEqual({ batchId: "B2", dsids: ["1000000001"] });
    expect(api.nextBody({}, "B2")).toEqual({ batchId: "B2", dsids: [] });
  });

  it("recognises the search endpoint only", () => {
    const api = load();
    expect(api.isSearchUrl("https://reportaproblem.apple.com/api/purchase/search")).toBe(true);
    expect(api.isSearchUrl("/api/purchase/search?x=1")).toBe(true);
    expect(api.isSearchUrl("/api/purchase/searchx")).toBe(false);
    expect(api.isSearchUrl("/api/other")).toBe(false);
  });

  it("follows nextBatchId and reports done on the last page", async () => {
    const api = load();
    const bodies: unknown[] = [];
    const sent: [string[], boolean, string | null][] = [];
    const pages: Record<string, unknown> = { B2: page(["2000002"], "B3"), B3: page(["3000003"], null) };
    await api.crawl(
      page(["1000001"], "B2"),
      { batchId: null, dsids: ["1"] },
      async (body) => {
        bodies.push(body);
        return pages[(body as { batchId: string }).batchId];
      },
      async (items, done, error) => sent.push([items.map((i) => i.adamId), done, error]),
      async () => undefined,
    );
    expect(bodies).toEqual([{ batchId: "B2", dsids: ["1"] }, { batchId: "B3", dsids: ["1"] }]);
    expect(sent).toEqual([[["1000001"], false, null], [["2000002"], false, null], [["3000003"], true, null]]);
  });

  it("reports api_changed and http errors and stops", async () => {
    const api = load();
    const sent: [number, boolean, string | null][] = [];
    const send = async (items: Item[], done: boolean, error: string | null) => sent.push([items.length, done, error]);
    await api.crawl({ nope: 1 }, {}, async () => ({}), send, async () => undefined);
    await api.crawl(page(["1000001"], "B2"), {}, async () => { throw new Error("401"); }, send, async () => undefined);
    await api.crawl(page(["1000001"], "B2"), {}, async () => ({ bad: true }), send, async () => undefined);
    expect(sent).toEqual([[0, true, "api_changed"], [1, false, null], [0, true, "http"], [1, false, null], [0, true, "api_changed"]]);
  });

  it("stops after 500 pages", async () => {
    const api = load();
    let calls = 0;
    let last: [boolean, string | null] | null = null;
    await api.crawl(page([], "B"), {}, async () => { calls++; return page([], "B"); }, async (_i, done, error) => { last = [done, error]; }, async () => undefined);
    expect(calls).toBe(499);
    expect(last).toEqual([true, null]);
  });
});
