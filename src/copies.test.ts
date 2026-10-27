import { describe, expect, it } from "vitest";
import { accountCopiesText, accountStatus, afterUseAccount, pickStatusText, showCopiesLink, staysOnRefusal, copiesWord, copyInstallButton, copyNotes, copyPhone, markCopyPhone, offersSkipSave, skipSaveNote, copyWarning, groupCopies, stepIndex } from "./copies";
import type { CopyDto, PhoneDto } from "./types";

const c = (name: string, purchaser: string | null, extra: Partial<CopyDto> = {}): CopyDto => ({
  path: `/lib/${name}.ipa`,
  track_id: null,
  bundle_id: `com.example.${name.toLowerCase()}`,
  name,
  version: "1.0",
  ipad_only: false,
  purchaser_masked: purchaser,
  ...extra,
});
const phone = (name: string, cls: "iPhone" | "iPad", target = true): PhoneDto => ({
  udid: name,
  name,
  ios_version: "27.0",
  device_class: cls,
  product_type: "",
  target,
});

describe("groupCopies", () => {
  const all = [c("Сбер", "me***@example.com"), c("Альфа", "me***@example.com"), c("ВК", "vf***@example.com"), c("Мусор", null)];
  it("groups by purchaser, unknown last, names sorted", () => {
    const g = groupCopies(all, "");
    expect(g.map((x) => x.title)).toEqual(["me***@example.com", "vf***@example.com", "Неизвестный Apple ID"]);
    expect(g[0].copies.map((x) => x.name)).toEqual(["Альфа", "Сбер"]);
  });
  it("search by name or bundle id, case-insensitive; empty groups drop", () => {
    expect(groupCopies(all, "сб").map((x) => x.title)).toEqual(["me***@example.com"]);
    expect(groupCopies(all, "COM.EXAMPLE.ВК")[0].copies[0].name).toBe("ВК");
    expect(groupCopies(all, "нет такого")).toEqual([]);
  });
});

describe("copyWarning", () => {
  const copy = c("Сбер", "me***@example.com");
  it("only when the phone's Apple ID is different", () => {
    expect(copyWarning(copy, { kind: "different", phone_masked: "vf***@example.com" })).toBe(
      "При первом запуске iPhone попросит войти в me***@example.com: нужен пароль этого Apple ID",
    );
    expect(copyWarning(copy, { kind: "same" })).toBeNull();
    expect(copyWarning(copy, { kind: "unknown" })).toBeNull();
    expect(copyWarning(copy, null)).toBeNull();
    expect(copyWarning(c("Мусор", null), { kind: "different", phone_masked: "x" })).toBeNull();
  });
});

describe("copyInstallButton", () => {
  it("names the phone", () => {
    expect(copyInstallButton(c("Сбер", null), phone("Max", "iPhone"))).toEqual({ text: "Поставить на Max", disabled: false });
  });
  it("iPad-only copy on an iPhone is disabled; on an iPad it is fine", () => {
    const pad = c("VK HD", null, { ipad_only: true });
    expect(copyInstallButton(pad, phone("Max", "iPhone"))).toEqual({ text: "Поставить на Max", disabled: true });
    expect(copyInstallButton(pad, phone("Планшет", "iPad")).disabled).toBe(false);
  });
  it("no phone or no copy selected", () => {
    expect(copyInstallButton(c("Сбер", null), undefined)).toEqual({ text: "Поставить на iPhone", disabled: true });
    expect(copyInstallButton(null, phone("Max", "iPhone"))).toEqual({ text: "Поставить на Max", disabled: true });
  });
});

describe("copyNotes", () => {
  it("version and iPad-only", () => {
    expect(copyNotes(c("A", null, { version: "2.1" }))).toEqual(["Версия 2.1"]);
    expect(copyNotes(c("A", null, { version: "", ipad_only: true }))).toEqual(["Только для iPad"]);
  });
});

describe("accounts", () => {
  it("copies word", () => {
    expect([1, 2, 5, 11, 21, 22, 25].map(copiesWord)).toEqual(["копия", "копии", "копий", "копий", "копия", "копии", "копий"]);
    expect(accountCopiesText(0)).toBe("Копий нет");
    expect(accountCopiesText(15)).toBe("15 копий");
  });
  it("status", () => {
    expect(accountStatus({ saved: true })).toBe("Вход сохранён");
    expect(accountStatus({ saved: false })).toBe("Нужен вход");
  });
});

describe("stepIndex", () => {
  it("Apple ID covers the list and the login form; copies are outside the steps", () => {
    expect(stepIndex("accounts")).toBe(0);
    expect(stepIndex("login")).toBe(0);
    expect(stepIndex("phone")).toBe(1);
    expect(stepIndex("done")).toBe(3);
    expect(stepIndex("copies")).toBe(-1);
    expect(stepIndex("warning")).toBe(-1);
  });
});

describe("showCopiesLink", () => {
  it("not on the first-run warning", () => {
    expect(showCopiesLink("warning")).toBe(false);
    expect(showCopiesLink("accounts")).toBe(true);
    expect(showCopiesLink("login")).toBe(true);
  });
});

describe("afterUseAccount", () => {
  const acc = { email: "me@example.com", name: "Me" };
  it("restored → iPhone; not restored → login; save failed → stay", () => {
    expect(afterUseAccount({ restored: true, account: acc, prefill: null, dropped: [], error: null })).toBe("phone");
    expect(afterUseAccount({ restored: false, account: null, prefill: "o@example.com", dropped: [], error: null })).toBe("login");
    const err = { code: "save_session_failed", text: "Не получилось сохранить вход текущего Apple ID", retryable: false };
    expect(afterUseAccount({ restored: false, account: acc, prefill: null, dropped: [], error: err })).toBe("stay");
  });
});

describe("switching accounts", () => {
  it("slow token check gets its own text", () => {
    expect(pickStatusText(false)).toBe("Входим…");
    expect(pickStatusText(true)).toBe("Apple отвечает медленно, проверяем вход…");
  });
  it("refusals keep the person where they are", () => {
    const e = (code: string) => ({ code, text: "", retryable: false });
    expect(staysOnRefusal(e("downloads_running"))).toBe(true);
    expect(staysOnRefusal(e("save_session_failed"))).toBe(true);
    expect(staysOnRefusal(e("unknown"))).toBe(false);
  });
});

describe("copy job ids", () => {
  it("the largest id Rust can return survives JSON as a JS number", () => {
    const id = -(2 ** 53 - 1);
    expect(Number.isSafeInteger(id)).toBe(true);
    expect(JSON.parse(JSON.stringify({ id })).id).toBe(id);
    // а 62-битное (как было) — уже нет
    expect(Number.isSafeInteger(-(2 ** 62))).toBe(false);
  });
});

describe("copyPhone", () => {
  const a = phone("A", "iPhone", true);
  const b = phone("B", "iPhone", false);
  it("starts from the main target, then keeps its own choice", () => {
    expect(copyPhone([a, b], null)?.udid).toBe("A");
    expect(copyPhone([a, b], "B")?.udid).toBe("B");
  });
  it("chosen phone unplugged → none, never silently another phone", () => {
    expect(copyPhone([a], "B")).toBeUndefined();
  });
  it("marks only the screen's own phone as target", () => {
    expect(markCopyPhone([a, b], "B").map((p) => p.target)).toEqual([false, true]);
    expect(markCopyPhone([a, b], null).map((p) => p.target)).toEqual([true, false]);
  });
});

describe("skip saving the previous login", () => {
  const e = (code: string) => ({ code, text: "x", retryable: false });
  it("offered only when the snapshot could not be saved", () => {
    expect(offersSkipSave(e("save_session_failed"))).toBe(true);
    expect(offersSkipSave(e("keychain_waiting"))).toBe(true);
    expect(offersSkipSave(e("wrong_credentials"))).toBe(false);
    expect(offersSkipSave(null)).toBe(false);
  });
  it("these refusals keep the person on the screen", () => {
    for (const c of ["keychain_waiting", "busy", "save_session_failed", "downloads_running"]) expect(staysOnRefusal(e(c))).toBe(true);
  });
  it("honest note, masked address, no password promise", () => {
    expect(skipSaveNote("us***@example.com")).toBe(
      "Можно продолжить без сохранения: для us***@example.com потом снова понадобятся пароль и код. Пароли программа не хранит.",
    );
    expect(skipSaveNote(null)).toBe("Можно продолжить без сохранения: для прежнего Apple ID потом снова понадобятся пароль и код. Пароли программа не хранит.");
  });
});
