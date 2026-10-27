import { describe, expect, it } from "vitest";
import { canPickVersion, downloadOnlyNote, findOffered, foundMessage, iosRunsOn, recheckFrom, splitStart, startVersions, tooOldNote, versionLabel, waitText } from "./versions";
import type { RowDto } from "./types";

const base: RowDto = {
  track_id: 5,
  bundle_id: "",
  name: "А",
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
  min_ios: null,
};

describe("iosRunsOn", () => {
  it("compares dotted and short versions like Rust", () => {
    expect(iosRunsOn("15.0", "15.8.8")).toBe(true);
    expect(iosRunsOn("16.3", "15.8.8")).toBe(false);
    expect(iosRunsOn("17", "17.0")).toBe(true);
    expect(iosRunsOn("26.0.1", "26")).toBe(false);
    expect(iosRunsOn("16.10", "16.3")).toBe(false);
    expect(iosRunsOn("?", "17")).toBeNull();
    expect(iosRunsOn("17", "")).toBeNull();
    expect(iosRunsOn("17..1", "17")).toBeNull();
  });
});

describe("tooOldNote", () => {
  it("speaks only when both known and phone is older", () => {
    expect(tooOldNote("16.0", "15.7")).toBe("Нужна iOS 16.0, на iPhone 15.7");
    expect(tooOldNote("15.0", "15.7")).toBeNull();
    expect(tooOldNote(null, "15.7")).toBeNull();
    expect(tooOldNote("16.0", null)).toBeNull();
    expect(tooOldNote("x", "15.7")).toBeNull();
  });
});

describe("splitStart", () => {
  it("too old for the phone goes download-only, the rest installs", () => {
    const old = { ...base, track_id: 1, min_ios: "17.0" };
    const ok = { ...base, track_id: 2, min_ios: "15.0" };
    const unknown = { ...base, track_id: 3 };
    const r = splitStart([old, ok, unknown], (x) => tooOldNote(x.min_ios, "16.1") !== null);
    expect(r.install.map((x) => x.track_id)).toEqual([2, 3]);
    expect(r.downloadOnly.map((x) => x.track_id)).toEqual([1]);
    expect(downloadOnlyNote("16.1")).toBe("Скачаем, но на этот iPhone не поставим — можно найти версию под iOS 16.1");
  });
});

describe("versions", () => {
  it("labels with date, or says it is not known yet", () => {
    expect(versionLabel({ id: "1", display: "12.15.0", released: "2022-03-18", checked: true })).toBe("12.15.0 · 18.03.2022");
    expect(versionLabel({ id: "1", display: "12.15.0", released: null, checked: true })).toBe("12.15.0");
    expect(versionLabel({ id: "847801982", display: null, released: null, checked: true })).toBe("Версия № 847801982 (Apple не назвал)");
    // ipatool не разобрал метаданные, но дату удалось достать из ошибки
    expect(versionLabel({ id: "847801982", display: null, released: "2022-04-04", checked: true })).toBe("Версия № 847801982 · 04.04.2022");
    // не спросили (пауза после 429) — так и говорим
    expect(versionLabel({ id: "847801982", display: null, released: null, checked: false })).toBe("Версия № 847801982 (не проверено)");
  });

  it("unasked versions are asked again from the first of them", () => {
    const v = (id: string, checked: boolean) => ({ id, display: checked ? id : null, released: null, checked });
    // от новой к старой: «before» — последняя проверенная перед первой непроверенной
    expect(recheckFrom([v("9", true), v("8", false), v("7", false)])).toEqual({ before: "9" });
    expect(recheckFrom([v("9", false), v("8", false)])).toEqual({ before: null });
    expect(recheckFrom([v("9", true)])).toBeNull();
  });

  it("countdown text after 429", () => {
    expect(waitText(42)).toBe("Apple просит подождать: можно снова через 42 с");
    expect(waitText(0)).toBeNull();
  });

  it("find is offered right on an incompatible tile", () => {
    const tb = { ...base, min_ios: "17.0" };
    expect(findOffered(tb, { kind: "yes" }, "16.1", undefined)).toBe(true);
    expect(findOffered(tb, { kind: "yes" }, "17.1", undefined)).toBe(false);
    expect(findOffered(tb, { kind: "yes" }, null, undefined)).toBe(false);
    expect(findOffered(tb, { kind: "yes" }, "16.1", { id: "1", display: "6.0" })).toBe(false);
    expect(findOffered({ ...tb, other_account: true }, { kind: "yes" }, "16.1", undefined)).toBe(false);
  });

  it("start sends chosen versions of the started rows only", () => {
    const chosen = new Map([
      [1, { id: "111", display: "1.0" }],
      [9, { id: "999", display: null }],
    ]);
    expect(startVersions([1, 2], chosen)).toEqual([[1, "111"]]);
  });

  it("a version can be picked for an owned, downloadable row", () => {
    expect(canPickVersion(base, { kind: "yes" })).toBe(true);
    expect(canPickVersion({ ...base, source: "removed" }, { kind: "yes" })).toBe(true);
    expect(canPickVersion({ ...base, source: "removed" }, { kind: "checking" })).toBe(false);
    expect(canPickVersion({ ...base, other_account: true }, { kind: "yes" })).toBe(false);
    expect(canPickVersion({ ...base, track_id: -3 }, { kind: "yes" })).toBe(false);
    expect(canPickVersion({ ...base, source: "saved" }, undefined)).toBe(false);
    expect(canPickVersion({ ...base, unknown_account: true }, { kind: "yes" })).toBe(false);
  });

  it("find result in words", () => {
    const v = { id: "7", display: "12.1.0", released: "2022-01-10", checked: true };
    const no = { exhausted: false, wait_s: null };
    expect(foundMessage({ ...no, version: v, next_ios: 16, none_older: false, error: null }, "15.8")).toBe(
      "12.1.0 вышла до iOS 16 — должна запуститься на iOS 15.8. Перед установкой проверим ещё раз.",
    );
    expect(foundMessage({ ...no, version: null, next_ios: 16, none_older: true, error: null }, "15.8")).toBe(
      "Все версии вышли после iOS 16: на iOS 15.8, скорее всего, не встанет ни одна.",
    );
    expect(foundMessage({ ...no, version: null, next_ios: null, none_older: false, error: null }, "26.1")).toBe(
      "На iPhone новейшая iOS — подойдёт последняя версия.",
    );
    expect(foundMessage({ ...no, version: null, next_ios: 16, none_older: false, error: null }, "15.8")).toBe(
      "Не удалось найти: Apple не дал даты версий. Выберите версию из списка.",
    );
    expect(foundMessage({ exhausted: true, wait_s: null, version: null, next_ios: 16, none_older: false, error: null }, "15.8")).toBe(
      "Проверили 10 версий, подходящей не нашли — попробуйте «Ещё старше»",
    );
    const err = { code: "apple_unavailable", text: "Apple просит подождать", retryable: true };
    expect(foundMessage({ ...no, version: null, next_ios: 16, none_older: false, error: err }, "15.8")).toBe("Apple просит подождать");
  });
});
