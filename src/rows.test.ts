import { describe, expect, it } from "vitest";
import {
  addLinkRow,
  applyOwnedRow,
  mergePurchases,
  applyOwned,
  canSelect,
  groupRows,
  iconFor,
  idsToCheck,
  initialOwned,
  matchesQuery,
  ownedView,
  recheckIds,
} from "./rows";
import type { OwnedEvent, RowDto } from "./types";

function row(id: number, name: string, over: Partial<RowDto> = {}): RowDto {
  return {
    track_id: id,
    bundle_id: "",
    name,
    version: "",
    removed_from_ru: false,
    has_saved_copy: false,
    owned: true,
    source: "purchase",
    letter: null,
    color: null,
    installed_version: null,
    available_version: null,
    install_state: "not_installed",
    checked: false,
    ipad_only: false,
    unknown_account: false,
    ...over,
  };
}
const cat = (id: number, name: string, over: Partial<RowDto> = {}) =>
  row(id, name, { source: "removed", owned: null, removed_from_ru: true, letter: name[0], color: "#21A038", ...over });

const rows: RowDto[] = [
  cat(1, "СберБанк Онлайн"),
  cat(2, "РЖД Пассажирам", { owned: false }),
  cat(3, "Т-Банк", { owned: true }),
  row(10, "Ёлка", { removed_from_ru: true }),
  row(11, "Telegram"),
  row(12, "com.example.copy", { source: "saved", has_saved_copy: true }),
];

const ev = (track_id: number, owned: boolean | null, error: OwnedEvent["error"] = null, over: Partial<OwnedEvent> = {}): OwnedEvent => ({
  track_id,
  owned,
  error,
  available_version: null,
  install_state: "not_installed",
  checked: false,
  ...over,
});

const stopErr = { code: "not_logged_in", text: "Вы не вошли в Apple ID. Войдите снова.", retryable: false };
const softErr = { code: "apple_unavailable", text: "Серверы Apple сейчас не отвечают. Попробуйте через 10–15 минут.", retryable: true };

describe("groupRows", () => {
  it("catalog and link rows first in their order, the rest below, filtered", () => {
    const g = groupRows(rows, "");
    expect(g.removed.map((r) => r.track_id)).toEqual([1, 2, 3]);
    expect(g.other.map((r) => r.track_id)).toEqual([10, 11, 12]);
  });
  it("search filters only the other purchases", () => {
    const g = groupRows(rows, "  tele ");
    expect(g.removed.map((r) => r.track_id)).toEqual([1, 2, 3]);
    expect(g.other.map((r) => r.track_id)).toEqual([11]);
  });
  it("link rows join the first group", () => {
    const withLink = addLinkRow(rows, row(50, "Новое", { source: "link" }));
    expect(groupRows(withLink, "").removed.map((r) => r.track_id)).toEqual([50, 1, 2, 3]);
  });
});

describe("matchesQuery", () => {
  it("ignores case and ё", () => {
    expect(matchesQuery("Ёлка", "елк")).toBe(true);
    expect(matchesQuery("елка", "ЁЛ")).toBe(true);
    expect(matchesQuery("Telegram", "")).toBe(true);
    expect(matchesQuery("Telegram", "vk")).toBe(false);
  });
});

describe("addLinkRow", () => {
  it("puts a new row on top", () => {
    const r = addLinkRow(rows, row(50, "Новое", { source: "link" }));
    expect(r[0].track_id).toBe(50);
    expect(r).toHaveLength(rows.length + 1);
  });
  it("does not duplicate a row that is already listed", () => {
    const r = addLinkRow(rows, cat(1, "СберБанк Онлайн", { owned: true }));
    expect(r).toHaveLength(rows.length);
    expect(r.find((x) => x.track_id === 1)?.owned).toBe(true);
  });
});

describe("owned states", () => {
  it("initial states come from the rows; unknown catalog rows are being checked", () => {
    const m = initialOwned(rows);
    expect(m.get(1)).toEqual({ kind: "checking" });
    expect(m.get(2)).toEqual({ kind: "no" });
    expect(m.get(3)).toEqual({ kind: "yes" });
    expect(m.get(11)).toEqual({ kind: "yes" });
  });
  it("only unknown catalog rows are sent to check_owned", () => {
    expect(idsToCheck(rows)).toEqual([1]);
  });
  it("events fill rows one by one", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б"), cat(3, "В")]);
    m = applyOwned(m, ev(1, true, null));
    m = applyOwned(m, ev(2, false, null));
    m = applyOwned(m, ev(3, null, softErr));
    expect(m.get(1)).toEqual({ kind: "yes" });
    expect(m.get(2)).toEqual({ kind: "no" });
    expect(m.get(3)).toEqual({ kind: "error", error: softErr });
  });
  it("a stopping error ends the check for every row still waiting", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б"), cat(3, "В", { owned: true })]);
    m = applyOwned(m, ev(1, null, stopErr));
    expect(m.get(1)).toEqual({ kind: "error", error: stopErr });
    expect(m.get(2)).toEqual({ kind: "error", error: stopErr });
    expect(m.get(3)).toEqual({ kind: "yes" });
  });
  it("a soft error leaves the others waiting", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б")]);
    m = applyOwned(m, ev(1, null, softErr));
    expect(m.get(2)).toEqual({ kind: "checking" });
  });
  it("recheck sends the row again together with the rows still waiting (a new call cancels the old one)", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б"), cat(3, "В")]);
    m = applyOwned(m, ev(1, null, softErr));
    expect(recheckIds(m, 1)).toEqual([1, 2, 3]);
  });
});

describe("ownedView and canSelect", () => {
  it("texts per state", () => {
    expect(ownedView({ kind: "checking" })).toEqual({ text: "Проверяем…", spinner: true, recheck: false });
    expect(ownedView({ kind: "yes" })).toEqual({ text: "Есть в ваших покупках", spinner: false, recheck: false });
    expect(ownedView({ kind: "no" })).toEqual({ text: "Не скачивалось с этого Apple ID", spinner: false, recheck: false });
    expect(ownedView({ kind: "error", error: softErr })).toEqual({ text: "Не удалось проверить", spinner: false, recheck: true });
  });
  it("catalog rows need a confirmed purchase or a saved copy", () => {
    expect(canSelect(cat(1, "А"), { kind: "checking" })).toBe(false);
    expect(canSelect(cat(1, "А"), { kind: "yes" })).toBe(true);
    expect(canSelect(cat(1, "А"), { kind: "no" })).toBe(false);
    expect(canSelect(cat(1, "А"), { kind: "error", error: softErr })).toBe(false);
    expect(canSelect(cat(1, "А", { has_saved_copy: true }), { kind: "no" })).toBe(true);
  });
  it("iPad-only rows are disabled when installing to an iPhone, enabled for an iPad or no phone", () => {
    const pad = cat(1, "ВКонтакте для iPad", { ipad_only: true, owned: true });
    expect(canSelect(pad, { kind: "yes" }, true)).toBe(false);
    expect(canSelect(pad, { kind: "yes" }, false)).toBe(true);
    expect(canSelect(row(11, "x", { ipad_only: true }), undefined, true)).toBe(false);
    expect(canSelect(cat(2, "Обычное"), { kind: "yes" }, true)).toBe(true);
  });
  it("purchases, links and saved copies are always selectable", () => {
    expect(canSelect(row(11, "Telegram"), undefined)).toBe(true);
    expect(canSelect(row(12, "x", { source: "saved" }), undefined)).toBe(true);
    expect(canSelect(row(50, "x", { source: "link" }), undefined)).toBe(true);
  });
});

describe("iconFor", () => {
  it("uses the catalog letter and colour", () => {
    expect(iconFor(cat(1, "СберБанк", { letter: "С", color: "#21A038" }))).toEqual({ letter: "С", color: "#21A038" });
  });
  it("derives a stable letter and colour for purchases", () => {
    const a = iconFor(row(11, "telegram"));
    expect(a.letter).toBe("T");
    expect(a.color).toMatch(/^#[0-9A-F]{6}$/i);
    expect(iconFor(row(11, "telegram"))).toEqual(a);
  });
  it("skips non-letters and handles an empty name", () => {
    expect(iconFor(row(11, "«2ГИС»")).letter).toBe("2");
    expect(iconFor(row(11, "")).letter).toBe("?");
  });
});

describe("progressive purchases", () => {
  it("keeps the catalog and link rows on screen and takes the rest from the purchases", () => {
    const now = [row(50, "Ссылка", { source: "link" }), cat(1, "СберБанк Онлайн", { owned: true }), row(12, "copy", { source: "saved" })];
    const incoming = [cat(1, "СберБанк Онлайн"), row(10, "Ёлка"), row(11, "Telegram")];
    const m = mergePurchases(now, incoming);
    // копия 12 не среди покупок — остаётся внизу
    expect(m.map((r) => r.track_id)).toEqual([50, 1, 10, 11, 12]);
    // на экране права уже проверены: строка каталога не откатывается к «Проверяем…»
    expect(m[1].owned).toBe(true);
  });
  it("saved copies that are not among the purchases stay, ones that are get replaced", () => {
    const now = [cat(1, "Сбер"), row(12, "copy", { source: "saved" }), row(13, "copy2", { source: "saved" })];
    const m = mergePurchases(now, [cat(1, "Сбер"), row(13, "Покупка")]);
    expect(m.map((r) => [r.track_id, r.source])).toEqual([[1, "removed"], [13, "purchase"], [12, "saved"]]);
  });
  it("a link row that also came as a purchase is not listed twice", () => {
    const m = mergePurchases([row(50, "Ссылка", { source: "link" })], [row(50, "Ссылка"), row(11, "Telegram")]);
    expect(m.map((r) => r.track_id)).toEqual([50, 11]);
  });
});

describe("applyOwnedRow", () => {
  it("brings the version, state and ownership into the row", () => {
    const r = applyOwnedRow([cat(1, "А"), cat(2, "Б")], ev(1, true, null, { available_version: "12.15.0", install_state: "update_available", checked: true }));
    expect(r[0]).toMatchObject({ owned: true, available_version: "12.15.0", install_state: "update_available" });
    expect(r[1]).toEqual(cat(2, "Б"));
  });
  it("an error keeps what the row already had", () => {
    const before = cat(1, "А", { owned: true, available_version: "1.0" });
    const r = applyOwnedRow([before], ev(1, null, softErr));
    expect(r[0]).toEqual(before);
  });
});

describe("unknown Apple ID copies", () => {
  it("stay on screen after purchases arrive and can be selected", () => {
    const u = row(5, "com.example.u", { source: "saved", has_saved_copy: true, unknown_account: true });
    const m = mergePurchases([u], [row(1, "Alpha")]);
    expect(m.map((r) => r.track_id)).toEqual([1, 5]);
    expect(canSelect(u, undefined, true)).toBe(true);
  });
});
