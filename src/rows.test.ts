import { describe, expect, it } from "vitest";
import {
  addLinkRow,
  applyOwnedRow,
  mergePurchases,
  applyOwned,
  canSelect,
  chipCounts,
  chipList,
  emptySearchText,
  foundText,
  groupPick,
  hasNewer,
  hiddenCount,
  iconFor,
  idsToCheck,
  initialOwned,
  isLight,
  matchesQuery,
  matchesRow,
  pendingChecks,
  recheckIds,
  rovingIndex,
  selectionLabel,
  selectionSummary,
  tileA11y,
  tileLine,
  visibleGroups,
  visibleChecks,
  replacePhoneRows,
  purchasesForSource,
  sourceChoices,
  SOURCE_HINT,
  HIDDEN_PURCHASES_HINT,
  NAMESAKE_NOTE,
  OTHER_ID_LOGIN_NOTE,
  autoChecks,
  barState,
  dropStoreRows,
  replaceStoreRows,
  storeTerm,
  NO_PHONE_HINT,
  isNewRow,
  retryCatalogErrors,
  showSweepButton,
  sweepText,
} from "./rows";
import type { OwnedEvent, PhoneDto, RowDto } from "./types";
import type { OwnedMap, OwnedState } from "./rows";

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
    aliases: [],
    category: null,
    size_bytes: null,
    other_account: false,
    purchaser_masked: null,
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

describe("visibleGroups", () => {
  it("catalog and link rows, then the phone group, then purchases and copies", () => {
    const all = [...rows, row(60, "Онлайн", { source: "phone", owned: null })];
    const g = visibleGroups(all, "all", "");
    expect(g.map((x) => [x.key, x.title, x.rows.map((r) => r.track_id)])).toEqual([
      ["catalog", "Каталог", [1, 2, 3]],
      ["phone", "С этого iPhone", [60]],
      ["buy", "Мои покупки", [10, 11, 12]],
    ]);
  });
  it("search applies to every group, the catalog too", () => {
    const g = visibleGroups(rows, "all", "  tele ");
    // от трёх букв — ещё и пустая группа «В App Store»
    expect(g.map((x) => x.rows.map((r) => r.track_id))).toEqual([[], [], [11], []]);
  });
  it("link rows join the catalog group", () => {
    const withLink = addLinkRow(rows, row(50, "Новое", { source: "link" }));
    expect(visibleGroups(withLink, "all", "")[0].rows.map((r) => r.track_id)).toEqual([50, 1, 2, 3]);
  });
  it("a chip narrows every group", () => {
    const list = [cat(1, "Сбер", { category: "bank" }), cat(2, "ВК", { category: "social" }), row(10, "Банк-покупка")];
    expect(visibleGroups(list, "bank", "").map((x) => x.rows.map((r) => r.track_id))).toEqual([[1], [], []]);
    expect(visibleGroups(list, "buy", "").map((x) => x.rows.map((r) => r.track_id))).toEqual([[], [], [10]]);
  });
});

describe("foundText", () => {
  it("counts what the chip and the search leave on screen", () => {
    expect(foundText(rows, "all", "")).toBe("Найдено 6");
    expect(foundText(rows, "all", "tele")).toBe("Найдено 1");
    expect(foundText(rows, "all", "zzz")).toBe("Найдено 0");
  });
});

describe("matchesRow", () => {
  const sbol = cat(2, "СБОЛ", { aliases: ["Сбер", "СберБанк Онлайн"] });
  it("matches the name or any alias, ignoring case and ё", () => {
    expect(matchesRow(sbol, "сбер")).toBe(true);
    expect(matchesRow(sbol, "БАНК онл")).toBe(true);
    expect(matchesRow(cat(3, "Ёлки", { aliases: ["Новогодние ёлки"] }), "новогодние елки")).toBe(true);
    expect(matchesRow(sbol, "втб")).toBe(false);
    expect(matchesRow(sbol, "   ")).toBe(true);
  });
});

describe("chips", () => {
  const list = [
    cat(1, "СберБанк Онлайн", { category: "bank", aliases: ["СБОЛ"] }),
    cat(2, "ВТБ Онлайн", { category: "bank", installed_version: "21.3", available_version: "22.1", install_state: "update_available" }),
    cat(3, "ВКонтакте", { category: "social", installed_version: "8.90", available_version: "8.90", install_state: "installed" }),
    cat(4, "РЖД Пассажирам", { category: "travel" }),
    cat(5, "Маруся", { category: "other" }),
    row(10, "Telegram", { installed_version: "12.1", available_version: null }),
    row(11, "Amperfy", { installed_version: "1.4", available_version: "1.5", install_state: "update_available" }),
    row(12, "copy", { source: "saved" }),
    row(60, "Онлайн", { source: "phone", installed_version: "14.2" }),
  ];
  it("counts every chip; purchases without a category only count in the non-category chips", () => {
    expect(chipCounts(list, "", true)).toEqual({ all: 9, bank: 2, social: 1, travel: 1, buy: 3, on: 5, new: 2 });
  });
  it("counts respect the search", () => {
    expect(chipCounts(list, "сбол", true)).toEqual({ all: 1, bank: 1, social: 0, travel: 0, buy: 0, on: 0, new: 0 });
    expect(chipCounts(list, "онлайн", true)).toEqual({ all: 3, bank: 2, social: 0, travel: 0, buy: 0, on: 2, new: 1 });
  });
  it("without a phone nothing is «on iPhone»", () => {
    expect(chipCounts(list, "", false)).toMatchObject({ on: 0, new: 0 });
    expect(chipList(false).map((c) => c.key)).toEqual(["all", "bank", "social", "travel", "buy"]);
    expect(chipList(true).map((c) => c.label)).toEqual(["Все", "Банки", "Соцсети и почта", "Транспорт и покупки", "Мои покупки", "На iPhone", "Есть новее"]);
  });
});

describe("hasNewer", () => {
  const v = (have: string | null, avail: string | null, install_state: RowDto["install_state"] = "update_available") =>
    row(1, "x", { installed_version: have, available_version: avail, install_state });
  it("only when both versions are known and differ", () => {
    expect(hasNewer(v("21.3", "22.1"))).toBe(true);
    expect(hasNewer(v("2.1", "2.1", "installed"))).toBe(false);
    expect(hasNewer(v(null, "2.1", "not_installed"))).toBe(false);
    expect(hasNewer(v("2.1", null, "installed"))).toBe(false);
    // не сравнить («50800» и «5.8.0»), но разные — показываем
    expect(hasNewer(v("50800", "5.8.0", "unknown"))).toBe(true);
    // на телефоне новее копии — это не «есть новее»
    expect(hasNewer(v("3.0", "2.0", "installed"))).toBe(false);
  });
});

describe("selection summary", () => {
  const MB = 1024 * 1024;
  it("sums known sizes, marks a partial sum with +, omits size when none is known", () => {
    expect(selectionSummary([])).toEqual({ count: 0, size: null });
    expect(selectionSummary([row(1, "a", { size_bytes: 410 * MB }), row(2, "b", { size_bytes: 390 * MB }), row(3, "c", { size_bytes: 350 * MB })])).toEqual({ count: 3, size: "≈ 1,1 ГБ" });
    expect(selectionSummary([row(1, "a", { size_bytes: 410 * MB }), row(2, "b")])).toEqual({ count: 2, size: "≈ 410 МБ+" });
    expect(selectionSummary([row(1, "a"), row(2, "b")])).toEqual({ count: 2, size: null });
    expect(selectionSummary([row(1, "a", { size_bytes: 3 * 1024 * MB })])).toEqual({ count: 1, size: "≈ 3,0 ГБ" });
    expect(selectionSummary([row(1, "a", { size_bytes: 1000 })])).toEqual({ count: 1, size: "≈ 1 МБ" });
  });
});

describe("hidden selection", () => {
  it("counts chosen rows the chip or the search hides", () => {
    const chosen = [cat(1, "Сбер", { category: "bank" }), cat(2, "ВК", { category: "social" }), row(10, "Telegram")];
    expect(hiddenCount(chosen, "all", "", true)).toBe(0);
    expect(hiddenCount(chosen, "bank", "", true)).toBe(2);
    expect(hiddenCount(chosen, "all", "сбер", true)).toBe(2);
  });
  it("bar label", () => {
    expect(selectionLabel(0, 0)).toBe("Ничего не выбрано");
    expect(selectionLabel(5, 0)).toBe("Выбрано 5");
    expect(selectionLabel(5, 3)).toBe("Выбрано 5 · 3 скрыто фильтром");
  });
});

describe("groupPick", () => {
  it("offers to select every selectable row, then to clear them", () => {
    const list = [row(1, "a"), row(2, "b"), row(3, "c", { ipad_only: true })];
    const ok = (r: RowDto) => !r.ipad_only;
    expect(groupPick(list, ok, new Set([1]))).toEqual({ ids: [1, 2], allOn: false });
    expect(groupPick(list, ok, new Set([1, 2]))).toEqual({ ids: [1, 2], allOn: true });
    expect(groupPick([row(3, "c", { ipad_only: true })], ok, new Set())).toEqual({ ids: [], allOn: false });
  });
  it("skips apps already installed at the same version, keeps ones with a newer version", () => {
    const list = [
      row(1, "стоит", { installed_version: "2.0", available_version: "2.0", install_state: "installed" }),
      row(2, "новее", { installed_version: "1.0", available_version: "2.0", install_state: "update_available" }),
      row(3, "нет", {}),
      row(4, "с телефона", { source: "phone", installed_version: "14.2", install_state: "installed" }),
    ];
    expect(groupPick(list, () => true, new Set())).toEqual({ ids: [2, 3], allOn: false });
    expect(groupPick(list, () => true, new Set([2, 3]))).toEqual({ ids: [2, 3], allOn: true });
  });
});

describe("emptySearchText", () => {
  it("points to the App Store link", () => {
    expect(emptySearchText(" Сбер ")).toBe("Ничего не нашлось по «Сбер». Попробуйте другое название или вставьте ссылку из App Store ниже.");
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
  it("app_gone is a final answer: not owned, with its own state, others keep waiting", () => {
    const gone = { code: "app_gone", text: "Apple больше не отдаёт это приложение для скачивания", retryable: false, ref_code: null };
    let m = initialOwned([cat(1, "А"), cat(2, "Б")]);
    m = applyOwned(m, ev(1, false, gone));
    expect(m.get(1)).toEqual({ kind: "gone" });
    expect(m.get(2)).toEqual({ kind: "checking" });
  });
  it("a soft error leaves the others waiting", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б")]);
    m = applyOwned(m, ev(1, null, softErr));
    expect(m.get(2)).toEqual({ kind: "checking" });
  });
  it("after purchases arrive the waiting catalog rows go again together with the new phone rows", () => {
    const list = [cat(1, "А"), cat(2, "Б", { owned: true }), row(60, "О", { source: "phone", owned: null }), row(61, "Ч", { source: "phone", owned: null, other_account: true }), row(10, "Покупка")];
    let m = initialOwned(list);
    expect(pendingChecks(list, m)).toEqual([1, 60]);
    m = applyOwned(m, ev(1, true, null));
    expect(pendingChecks(list, m)).toEqual([60]);
  });
  it("recheck sends the row again together with the rows still waiting (a new call cancels the old one)", () => {
    let m = initialOwned([cat(1, "А"), cat(2, "Б"), cat(3, "В")]);
    m = applyOwned(m, ev(1, null, softErr));
    expect(recheckIds(m, 1)).toEqual([1, 2, 3]);
  });
});

describe("visibleChecks", () => {
  it("sends Rust only the shown rows still being checked, in screen order", () => {
    const list = [cat(1, "Альфа"), cat(2, "Бета"), cat(3, "Альт", { owned: true }), row(60, "Альбом", { source: "phone", owned: null })];
    const m = initialOwned(list);
    expect(visibleChecks(visibleGroups(list, "all", "аль"), m)).toEqual([1, 60]);
    expect(visibleChecks(visibleGroups(list, "all", "бета"), m)).toEqual([2]);
    expect(visibleChecks(visibleGroups(list, "all", "бета"), applyOwned(m, ev(2, true, null)))).toEqual([]);
  });
});

describe("tileLine and canSelect", () => {
  const yes = { kind: "yes" } as const;
  it("ownership state first for catalog and phone rows", () => {
    expect(tileLine(cat(1, "А"), { kind: "checking" }, false, true)).toEqual({ text: "проверяем…", tone: "checking" });
    expect(tileLine(cat(1, "А"), { kind: "no" }, false, true)).toEqual({ text: "Нет на вашем Apple ID", tone: "muted" });
    expect(tileLine(row(60, "А", { source: "phone" }), { kind: "no" }, false, true)).toEqual({ text: "Нет на вашем Apple ID", tone: "muted" });
    expect(tileLine(cat(1, "А"), { kind: "error", error: softErr }, false, true)).toEqual({ text: "Не удалось проверить · нажмите, чтобы повторить", tone: "retry" });
    // Apple больше не отдаёт: не выбрать и не повторять
    const gone = { kind: "gone" } as const;
    expect(tileLine(cat(1, "А"), gone, false, true)).toEqual({ text: "Apple больше не отдаёт", tone: "muted" });
    expect(tileLine(row(70, "С", { source: "store" }), gone, false, true)).toEqual({ text: "Apple больше не отдаёт", tone: "muted" });
    expect(canSelect(cat(1, "А"), gone)).toBe(false);
    // тёзка из App Store: подсказка третьей строкой; у каталога её нет
    expect(tileLine(row(70, "Сила звука", { source: "store" }), { kind: "no" }, false, true)).toEqual({ text: "Нет на вашем Apple ID", tone: "muted", note: NAMESAKE_NOTE });
    // копия на компьютере: ставится и без подтверждения прав
    expect(tileLine(cat(1, "А", { has_saved_copy: true, available_version: "16.4" }), { kind: "checking" }, false, true)).toEqual({ text: "доступно 16.4", tone: "plain", note: "Есть копия на компьютере" });
  });
  it("versions: newer, installed, available", () => {
    const vtb = cat(2, "ВТБ", { installed_version: "21.3", available_version: "22.1", install_state: "update_available" });
    expect(tileLine(vtb, yes, false, true)).toEqual({ text: "на iPhone 21.3 → ", up: "22.1", tone: "plain" });
    expect(tileLine(vtb, yes, false, false)).toEqual({ text: "доступно 22.1", tone: "plain" });
    expect(tileLine(cat(3, "А", { installed_version: "14.0", available_version: "14.0", install_state: "installed" }), yes, false, true)).toEqual({ text: "на iPhone 14.0", tone: "plain" });
    expect(tileLine(row(60, "О", { source: "phone", installed_version: "14.2" }), yes, false, true)).toEqual({ text: "на iPhone 14.2", tone: "plain" });
    expect(tileLine(row(10, "Т", { available_version: "7.12" }), undefined, false, true)).toEqual({ text: "доступно 7.12", tone: "plain" });
  });
  it("blocking reasons win", () => {
    expect(tileLine(cat(1, "VK для iPad", { ipad_only: true }), yes, true, true)).toEqual({ text: "только для iPad", tone: "warn" });
    expect(tileLine(row(60, "О", { source: "phone", other_account: true, installed_version: "1" }), undefined, false, true)).toEqual({ text: "Куплено на другом Apple ID", tone: "warn" });
  });
  it("an unknown Apple ID copy says so even when its version is known (Rust fills available_version from the copy)", () => {
    const u = row(5, "com.example.u", { source: "saved", unknown_account: true, has_saved_copy: true, available_version: "3.2" });
    expect(tileLine(u, undefined, false, true)).toMatchObject({ text: "доступно 3.2", note: "Неизвестный Apple ID" });
    expect(tileLine({ ...u, installed_version: "3.1" }, undefined, false, true)).toMatchObject({ note: "Неизвестный Apple ID" });
  });
  it("a saved copy is mentioned next to the version line", () => {
    const sber = cat(1, "Сбер", { has_saved_copy: true, available_version: "16.4" });
    expect(tileLine(sber, { kind: "yes" }, false, true)).toEqual({ text: "доступно 16.4", tone: "plain", note: "Есть копия на компьютере" });
    expect(tileLine({ ...sber, installed_version: "16.4", install_state: "installed" }, { kind: "yes" }, false, true)).toMatchObject({ text: "на iPhone 16.4", note: "Есть копия на компьютере" });
    expect(tileLine(row(10, "Т", { available_version: "7.12" }), undefined, false, true).note).toBeUndefined();
  });
  it("fallbacks without versions", () => {
    expect(tileLine(row(12, "c", { source: "saved", has_saved_copy: true }), undefined, false, true)).toEqual({ text: "Есть копия на компьютере", tone: "plain" });
    expect(tileLine(row(10, "c", { removed_from_ru: true }), yes, false, true)).toEqual({ text: "нет в российском App Store", tone: "muted" });
    expect(tileLine(row(12, "c", { source: "saved", unknown_account: true, has_saved_copy: true }), undefined, false, true)).toEqual({ text: "Неизвестный Apple ID", tone: "muted" });
    expect(tileLine(row(10, "c"), yes, false, true)).toEqual({ text: "в ваших покупках", tone: "muted" });
  });
  it("phone rows need a confirmed purchase and the same Apple ID", () => {
    const p = row(60, "О", { source: "phone", owned: null });
    expect(canSelect(p, { kind: "checking" })).toBe(false);
    expect(canSelect(p, { kind: "yes" })).toBe(true);
    expect(canSelect(p, { kind: "no" })).toBe(false);
    expect(canSelect({ ...p, other_account: true }, { kind: "yes" })).toBe(false);
    expect(idsToCheck([p, { ...p, track_id: 61, other_account: true }, { ...p, track_id: 62, owned: true }])).toEqual([60]);
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

describe("tileA11y", () => {
  it("selectable tiles are toggles", () => {
    expect(tileA11y("ВТБ", { text: "доступно 22.1", tone: "plain" }, true, true)).toEqual({ pressed: "true", disabled: false, label: null });
    expect(tileA11y("ВТБ", { text: "доступно 22.1", tone: "plain" }, true, false)).toEqual({ pressed: "false", disabled: false, label: null });
  });
  it("blocked tiles stay focusable, say why and never show as pressed", () => {
    expect(tileA11y("Чужое", { text: "Куплено на другом Apple ID", tone: "warn" }, false, true)).toEqual({
      pressed: "false",
      disabled: true,
      label: "Чужое, не выбрать: Куплено на другом Apple ID",
    });
  });
  it("a retry tile is a plain button, not a toggle", () => {
    expect(tileA11y("Маруся", { text: "Не удалось проверить · нажмите, чтобы повторить", tone: "retry" }, false, false)).toEqual({ pressed: null, disabled: false, label: null });
  });
});

describe("rovingIndex", () => {
  it("arrows move through the chips and wrap, Home and End jump, other keys do nothing", () => {
    expect(rovingIndex("ArrowRight", 0, 7)).toBe(1);
    expect(rovingIndex("ArrowRight", 6, 7)).toBe(0);
    expect(rovingIndex("ArrowLeft", 0, 7)).toBe(6);
    expect(rovingIndex("Home", 4, 7)).toBe(0);
    expect(rovingIndex("End", 1, 7)).toBe(6);
    expect(rovingIndex("Enter", 1, 7)).toBeNull();
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
  it("light backgrounds get dark ink", () => {
    expect(isLight("#ffdd2d")).toBe(true);
    expect(isLight("#FFFFFF")).toBe(true);
    expect(isLight("#21A038")).toBe(false);
    expect(isLight("#0a2973")).toBe(false);
    expect(isLight("oops")).toBe(false);
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

describe("phone as a source", () => {
  const ph = (udid: string, name: string, target = false): PhoneDto => ({ udid, name, ios_version: "17", device_class: "iPhone", product_type: "p", target });
  it("the foreign purchaser shows only the mask from Rust", () => {
    const r = row(60, "О", { source: "phone", other_account: true, purchaser_masked: "ma***@example.com" });
    expect(tileLine(r, undefined, false, true)).toEqual({ text: "Куплено на другом Apple ID (ma***@example.com)", tone: "warn" });
  });
  it("a source app missing on the target shows the source version", () => {
    const r = row(60, "О", { source: "phone", version: "5.2", installed_version: null, available_version: "5.3" });
    expect(tileLine(r, { kind: "yes" }, false, true)).toEqual({ text: "с телефона 5.2", tone: "plain" });
    expect(tileLine(r, { kind: "yes" }, false, false)).toEqual({ text: "с телефона 5.2", tone: "plain" });
    expect(tileLine({ ...r, installed_version: "5.0" }, { kind: "yes" }, false, true).text).toBe("на iPhone 5.0 → ");
  });
  it("picker only with more than one phone; target is marked", () => {
    expect(sourceChoices([ph("A", "Новый", true)], "A")).toBeNull();
    expect(sourceChoices([], null)).toBeNull();
    expect(sourceChoices([ph("A", "Новый", true), ph("B", "Старый")], "B")).toEqual([
      { udid: "A", label: "Новый (сюда ставим)", selected: false },
      { udid: "B", label: "Старый", selected: true },
    ]);
  });
  it("new source rows replace only the phone group", () => {
    const list = [row(1, "К", { source: "removed" }), row(60, "Старое", { source: "phone" }), row(10, "П")];
    const next = replacePhoneRows(list, [row(70, "Новое", { source: "phone" })]);
    expect(next.map((r) => r.track_id)).toEqual([1, 10, 70]);
    // строка группы с тем же trackId, что уже есть вне группы, не дублируется
    expect(replacePhoneRows(list, [row(10, "П", { source: "phone" })]).map((r) => r.track_id)).toEqual([1, 10]);
  });
  it("texts", () => {
    expect(SOURCE_HINT).toBe("Само приложение с телефона не забрать — копию скачаем с Apple по номеру. Работает, если приложение есть на Apple ID");
    expect(NAMESAKE_NOTE).toBe("Если у вас было приложение с таким названием, это могло быть другое — удалённое или скрытое");
    expect(HIDDEN_PURCHASES_HINT).toBe("Скрытые покупки: App Store → аватар → ваше имя → Скрытые покупки → Показать");
    expect(OTHER_ID_LOGIN_NOTE).toBe("Войдите в Apple ID, с которого скачано это приложение");
  });
});

describe("purchases built for another source", () => {
  const list = [row(1, "К", { source: "removed" }), row(60, "С нового", { source: "phone" })];
  const incoming = [row(1, "К", { source: "removed" }), row(10, "П"), row(70, "Со старого", { source: "phone" })];
  it("matching tag or no explicit choice: the event is taken as is", () => {
    expect(purchasesForSource(list, incoming, "OLD", null)).toEqual({ rows: incoming, stale: false });
    expect(purchasesForSource(list, incoming, "NEW", "NEW")).toEqual({ rows: incoming, stale: false });
  });
  it("tag differs from the chosen source: phone rows on screen stay, the rest is new", () => {
    const r = purchasesForSource(list, incoming, "OLD", "NEW");
    expect(r.stale).toBe(true);
    expect(r.rows.map((x) => x.track_id)).toEqual([1, 10, 60]);
    // без группы (Lookup упал) — тоже не тот источник: перестроить
    expect(purchasesForSource(list, incoming, null, "NEW").stale).toBe(true);
  });
});

describe("«В App Store»", () => {
  const store = (id: number, name: string, term: string, over: Partial<RowDto> = {}) => row(id, name, { source: "store", owned: null, store_term: term, ...over });

  it("storeTerm: от трёх букв, без пробелов по краям", () => {
    expect(storeTerm("  сб ")).toBeNull();
    expect(storeTerm("")).toBeNull();
    expect(storeTerm(" Сбер ")).toBe("Сбер");
  });

  it("группа есть только при запросе от трёх букв и показывает ответ на этот запрос", () => {
    const list = [cat(1, "Сбербанк"), store(20, "Пример Банк", "сбер"), store(21, "Старое", "почта")];
    expect(visibleGroups(list, "all", "").map((g) => g.key)).toEqual(["catalog", "phone", "buy"]);
    expect(visibleGroups(list, "all", "сб").map((g) => g.key)).toEqual(["catalog", "phone", "buy"]);
    const g = visibleGroups(list, "all", "Сбер ");
    expect(g.map((x) => x.key)).toEqual(["catalog", "phone", "buy", "store"]);
    expect(g[3]).toMatchObject({ title: "В App Store" });
    // имя не совпадает, но это ответ App Store на тот же запрос; ответ на прежний — скрыт
    expect(g[3].rows.map((r) => r.track_id)).toEqual([20]);
    expect(g[2].rows).toEqual([]);
    expect(hiddenCount([list[2]], "all", "сбер", false)).toBe(1);
    expect(chipCounts(list, "сбер", false).all).toBe(2);
    expect(visibleGroups(list, "buy", "сбер")[3].rows).toEqual([]);
  });

  it("replaceStoreRows: новый ответ вместо прежнего, выбранные остаются, повторы со строками экрана — нет", () => {
    const cur = [cat(1, "Сбер"), row(2, "Покупка"), store(20, "Выбрана", "сбер"), store(21, "Не выбрана", "сбер")];
    const incoming = [row(1, "Сбер", { source: "store" }), row(2, "x", { source: "store" }), row(30, "Новая", { source: "store" }), row(20, "Выбрана", { source: "store" })];
    const got = replaceStoreRows(cur, incoming, new Set([20]), "почта");
    expect(got.map((r) => [r.track_id, r.store_term ?? null])).toEqual([[1, null], [2, null], [20, "сбер"], [30, "почта"]]);
    expect(dropStoreRows(got, new Set([20])).map((r) => r.track_id)).toEqual([1, 2, 20]);
  });

  it("покупки не стирают строки App Store, а свои строки берут вместо них", () => {
    const cur = [store(20, "A", "сбер"), store(21, "B", "сбер")];
    expect(mergePurchases(cur, [row(21, "B")]).map((r) => [r.track_id, r.source])).toEqual([[21, "purchase"], [20, "store"]]);
  });

  it("права проверяются, выбрать можно только купленное, сама не отмечается", () => {
    const s = store(20, "A", "сбер");
    expect(idsToCheck([s])).toEqual([20]);
    expect(pendingChecks([s], new Map([[20, { kind: "checking" }]]))).toEqual([20]);
    expect(canSelect(s, { kind: "checking" })).toBe(false);
    expect(canSelect(s, { kind: "no" })).toBe(false);
    expect(canSelect(s, { kind: "yes" })).toBe(true);
    expect(tileLine(s, { kind: "no" }, false, false)).toEqual({ text: "Нет на вашем Apple ID", tone: "muted", note: NAMESAKE_NOTE });
    expect(tileLine({ ...s, available_version: "2.0" }, { kind: "yes" }, false, false)).toEqual({ text: "доступно 2.0", tone: "plain" });
    expect(autoChecks(s)).toBe(false);
    expect(autoChecks(cat(1, "Сбер"))).toBe(true);
  });
});

describe("barState", () => {
  it("ничего не выбрано: обе кнопки неактивны", () => {
    expect(barState(0, true)).toEqual({ go: { text: "Вернуть", disabled: true }, only: { text: "Только скачать", disabled: true }, hint: null });
  });
  it("с телефоном активны обе", () => {
    expect(barState(3, true)).toEqual({ go: { text: "Вернуть 3", disabled: false }, only: { text: "Только скачать 3", disabled: false }, hint: null });
  });
  it("без телефона «Вернуть» неактивна с подсказкой, «Только скачать» — активна", () => {
    expect(barState(2, false)).toEqual({ go: { text: "Вернуть 2", disabled: true }, only: { text: "Только скачать 2", disabled: false }, hint: NO_PHONE_HINT });
  });
});

describe("«Проверить весь каталог» и «Новое»", () => {
  it("sweep text and button", () => {
    expect(sweepText(null)).toBeNull();
    expect(sweepText({ checked: 3, total: 10, running: true })).toBe("Проверено 3 из 10");
    expect(sweepText({ checked: 0, total: 0, running: true })).toBe("Проверяем…");
    expect(sweepText({ checked: 10, total: 10, running: false })).toBe("Весь каталог проверен");
    expect(sweepText({ checked: 4, total: 10, running: false })).toBe("Проверено 4 из 10");
    expect(showSweepButton(null, true)).toBe(true);
    expect(showSweepButton(null, false)).toBe(false);
    expect(showSweepButton({ checked: 3, total: 10, running: true }, true)).toBe(false);
    expect(showSweepButton({ checked: 4, total: 10, running: false }, true)).toBe(true);
    expect(showSweepButton({ checked: 10, total: 10, running: false }, true)).toBe(false);
  });

  it("«Новое» only on catalog rows signalled by Rust", () => {
    const ids = new Set([5, 6]);
    expect(isNewRow(row(5, "А", { source: "removed" }), ids)).toBe(true);
    expect(isNewRow(row(6, "Б", { source: "purchase" }), ids)).toBe(false);
    expect(isNewRow(row(7, "В", { source: "removed" }), ids)).toBe(false);
  });

  it("sweep retries failed catalog rows only", () => {
    const error = { kind: "error" as const, error: { code: "unknown", text: "x", retryable: true } };
    const all = [row(1, "А", { source: "removed" }), row(2, "Б", { source: "removed" }), row(3, "В", { source: "store" }), row(4, "Г", { source: "removed" })];
    const before: OwnedMap = new Map<number, OwnedState>([
      [1, error],
      [2, { kind: "no" as const }],
      [3, error],
    ]);
    const after = retryCatalogErrors(all, before);
    expect(after.get(1)).toEqual({ kind: "checking" });
    expect(after.get(2)).toEqual({ kind: "no" });
    expect(after.get(3)).toEqual(error);
    expect(after.has(4)).toBe(false);
    expect(before.get(1)).toEqual(error);
  });
});
