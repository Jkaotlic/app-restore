import { describe, expect, it } from "vitest";
import { HISTORY_HINT, HISTORY_STEPS, historyProgressText, historyResultText, mergeHistoryRows } from "./history";
import { autoChecks, canSelect, idsToCheck, mergePurchases, pendingChecks, tileLine, visibleGroups, initialOwned } from "./rows";
import type { RowDto } from "./types";

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
    min_ios: null,
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
const hist = (id: number, name: string, over: Partial<RowDto> = {}) => row(id, name, { source: "history", owned: null, ...over });

describe("history group", () => {
  it("appears after purchases only when there are history rows", () => {
    expect(visibleGroups([row(10, "A")], "all", "").map((g) => g.key)).toEqual(["catalog", "phone", "buy"]);
    const g = visibleGroups([row(10, "A"), hist(20, "Сила")], "all", "");
    expect(g.map((x) => [x.key, x.title, x.rows.map((r) => r.track_id)])).toEqual([
      ["catalog", "Каталог", []],
      ["phone", "С этого iPhone", []],
      ["buy", "Мои покупки", [10]],
      ["history", "Из истории покупок", [20]],
    ]);
    expect(visibleGroups([hist(20, "Сила")], "all", "сила").map((x) => x.key)).toEqual(["catalog", "phone", "buy", "history", "store"]);
  });

  it("rows are checked, selectable only when owned and never auto-checked", () => {
    const r = hist(20, "Сила");
    expect(idsToCheck([r])).toEqual([20]);
    expect(pendingChecks([r], initialOwned([r]))).toEqual([20]);
    expect(canSelect(r, { kind: "checking" })).toBe(false);
    expect(canSelect(r, { kind: "yes" })).toBe(true);
    expect(autoChecks(r)).toBe(false);
    expect(tileLine(r, { kind: "checking" }, false, false).tone).toBe("checking");
    expect(tileLine(r, { kind: "no" }, false, false).text).toBe("Нет на вашем Apple ID");
  });

  it("survive a later purchases load unless purchases now have them", () => {
    const merged = mergePurchases([hist(20, "Сила"), hist(21, "Б")], [row(21, "Б")]);
    expect(merged.map((r) => [r.track_id, r.source])).toEqual([[21, "purchase"], [20, "history"]]);
  });

  it("adds only rows that are not on screen yet", () => {
    const merged = mergeHistoryRows([row(10, "A"), hist(20, "Сила")], [hist(10, "A"), hist(20, "Сила"), hist(30, "Новое")]);
    expect(merged.map((r) => r.track_id)).toEqual([10, 20, 30]);
  });
});

describe("history texts", () => {
  it("progress uses Russian plurals", () => {
    expect(historyProgressText(0)).toBe("Окно Apple открыто — войдите своим Apple ID…");
    expect(historyProgressText(1)).toBe("Найдено 1 приложение…");
    expect(historyProgressText(3)).toBe("Найдено 3 приложения…");
    expect(historyProgressText(11)).toBe("Найдено 11 приложений…");
    expect(historyProgressText(21)).toBe("Найдено 21 приложение…");
    expect(historyProgressText(125)).toBe("Найдено 125 приложений…");
  });

  it("result says how many are new", () => {
    expect(historyResultText(10, 0)).toBe("Готово: в истории 10 приложений — все уже есть в списке");
    // группа стоит выше блока «Не нашли приложение?»: место не называем, к ней ведёт «Показать»
    expect(historyResultText(10, 2)).toBe("Готово: в истории 10 приложений, новых 2 — в группе «Из истории покупок»");
    expect(historyResultText(1222, 433)).toBe("Готово: в истории 1222 приложения, новых 433 — в группе «Из истории покупок»");
    expect(historyResultText(0, 0)).toBe("Готово: в истории покупок приложений для iPhone не нашлось");
  });

  it("three steps before the Apple window opens, in order", () => {
    expect(HISTORY_STEPS).toEqual([
      "Войдите своим Apple ID на странице Apple",
      "Пролистайте список покупок вниз до самого конца — так Apple подгружает все приложения",
      "Окно закроется само, найденное появится здесь",
    ]);
  });

  it("hint", () => {
    expect(HISTORY_HINT).toBe("Скрыто или пропало из списка покупок. Скачать можно, если Apple ещё отдаёт приложение");
  });
});
