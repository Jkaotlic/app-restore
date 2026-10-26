import { describe, expect, it } from "vitest";
import { accountBlock, afterLoginScreen, askAppleIdOnDone, sameFromMatch, unknownAccountNote, canProceed, chooserNote, installsToIphone, showSkip, doneText, installCaption, isFinished, jobView, otherIdText, phoneView, purchasesProgressText, versionLine } from "./labels";
import type { JobDto, PhoneDto } from "./types";

const err = { code: "device_locked", text: "Разблокируйте iPhone и нажмите «Доверять»", retryable: false };

describe("jobView", () => {
  it("queued", () => {
    expect(jobView({ kind: "queued" }, null)).toEqual({ text: "В очереди", bar: 0, retry: false });
  });
  it("downloading shows megabytes and an indeterminate bar", () => {
    expect(jobView({ kind: "downloading", bytes: 0 }, null)).toEqual({ text: "Скачано 0 МБ", bar: "indeterminate", retry: false });
    expect(jobView({ kind: "downloading", bytes: 157 * 1024 * 1024 + 300_000 }, null).text).toBe("Скачано 157 МБ");
  });
  it("saved waits for the phone", () => {
    expect(jobView({ kind: "saved" }, null)).toEqual({ text: "Скачано, ждём iPhone", bar: null, retry: false });
  });
  it("installing is a determinate bar", () => {
    expect(jobView({ kind: "installing", percent: 37 }, null)).toEqual({ text: "Устанавливается, 37%", bar: 37, retry: false });
  });
  it("installed, plain and from a saved copy", () => {
    expect(jobView({ kind: "installed" }, null)).toEqual({ text: "Установлено", bar: null, retry: false });
    expect(jobView({ kind: "installed" }, "12.15.0").text).toBe("Поставлена сохранённая копия 12.15.0");
  });
  it("failed shows the error text and offers retry", () => {
    expect(jobView({ kind: "failed", error: err, saved: true }, null)).toEqual({ text: err.text, bar: null, retry: true });
  });
});

describe("isFinished", () => {
  it("installed and failed are final, the rest are not", () => {
    const states: [JobDto, boolean][] = [
      [{ kind: "queued" }, false],
      [{ kind: "downloading", bytes: 1 }, false],
      [{ kind: "saved" }, false],
      [{ kind: "installing", percent: 99 }, false],
      [{ kind: "installed" }, true],
      [{ kind: "failed", error: err, saved: false }, true],
    ];
    for (const [s, f] of states) expect(isFinished(s)).toBe(f);
  });
});

describe("doneText", () => {
  const email = "friend@users.noreply.github.com";
  it("same Apple ID: just installed", () => {
    expect(doneText({ kind: "installed" }, null, true, email)).toBe("Установлено");
  });
  it("other Apple ID: says which ID to sign in to", () => {
    expect(doneText({ kind: "installed" }, null, false, email)).toBe(`Установлено. При первом запуске войдите в ${email}`);
  });
  it("other Apple ID without a known email", () => {
    expect(doneText({ kind: "installed" }, null, false, null)).toBe(
      "Установлено. При первом запуске войдите в Apple ID, с которого скачивали",
    );
  });
  it("saved copy keeps its version", () => {
    expect(doneText({ kind: "installed" }, "9.1", false, email)).toBe(
      `Поставлена сохранённая копия 9.1. При первом запуске войдите в ${email}`,
    );
  });
  it("Apple ID not answered yet: neutral «Установлено», no hint", () => {
    expect(doneText({ kind: "installed" }, null, null, email)).toBe("Установлено");
  });
  it("not finished or failed falls back to the job label", () => {
    expect(doneText({ kind: "failed", error: err, saved: false }, null, false, email)).toBe(err.text);
    expect(doneText({ kind: "queued" }, null, true, email)).toBe("В очереди");
  });
});

describe("phoneView", () => {
  const p: PhoneDto = { udid: "u1", name: "iPhone Ивана", ios_version: "26.1", device_class: "iPhone", product_type: "iPhone17,1", target: true };
  it("iPhone", () => {
    expect(phoneView(p)).toEqual({ title: "iPhone Ивана", subtitle: "Подключён, iOS 26.1", isIpad: false });
  });
  it("iPad is labelled as such", () => {
    expect(phoneView({ ...p, name: "Планшет", device_class: "iPad" })).toEqual({
      title: "Планшет",
      subtitle: "iPad, iPadOS 26.1",
      isIpad: true,
    });
  });
  it("missing version", () => {
    expect(phoneView({ ...p, ios_version: "" }).subtitle).toBe("Подключён");
  });
});

describe("versionLine", () => {
  const v = (installed: string | null, available: string | null) => ({ installed_version: installed, available_version: available });
  it("with a phone: both versions", () => {
    expect(versionLine(v("12.14.0", "12.15.0"), true)).toBe("На iPhone: 12.14.0 · Доступно: 12.15.0");
  });
  it("installed, available not known yet", () => {
    expect(versionLine(v("12.14.0", null), true)).toBe("На iPhone: 12.14.0");
  });
  it("installed without a version string", () => {
    expect(versionLine(v("", "1.0"), true)).toBe("На iPhone: есть · Доступно: 1.0");
  });
  it("not on the phone: only what is available", () => {
    expect(versionLine(v(null, "12.15.0"), true)).toBe("Доступно: 12.15.0");
    expect(versionLine(v(null, null), true)).toBeNull();
  });
  it("without a phone there is no «На iPhone» part", () => {
    expect(versionLine(v("12.14.0", "12.15.0"), false)).toBe("Доступно: 12.15.0");
    expect(versionLine(v("12.14.0", null), false)).toBeNull();
  });
});

describe("installCaption", () => {
  it("installed and update available, only with a phone", () => {
    expect(installCaption("installed", true)).toBe("Уже стоит на iPhone");
    expect(installCaption("update_available", true)).toBe("Есть версия новее");
    expect(installCaption("not_installed", true)).toBeNull();
    expect(installCaption("unknown", true)).toBeNull();
    expect(installCaption("installed", false)).toBeNull();
  });
});

describe("purchasesProgressText", () => {
  it("before the first page and per page", () => {
    expect(purchasesProgressText(null)).toBe("Загружаем покупки");
    expect(purchasesProgressText({ page: 3, pages: 8 })).toBe("Загружаем покупки: страница 3 из 8");
  });
});

describe("chooserNote", () => {
  const dev = (cls: string, target = false): PhoneDto => ({ udid: cls + target, name: "n", ios_version: "26", device_class: cls, product_type: "p", target });
  it("one iPad that is not the target", () => {
    expect(chooserNote([dev("iPad")])).toBe("Это iPad. Подключите iPhone или выберите iPad");
  });
  it("several devices", () => {
    expect(chooserNote([dev("iPhone"), dev("iPad")])).toBe("Подключено несколько устройств. Выберите, куда ставить.");
  });
  it("one device chosen already or none at all: no note", () => {
    expect(chooserNote([dev("iPhone", true)])).toBeNull();
    expect(chooserNote([dev("iPad", true)])).toBeNull();
    expect(chooserNote([])).toBeNull();
  });
});

describe("otherIdText", () => {
  it("with the email and without it, no stray space before the comma", () => {
    expect(otherIdText("friend@users.noreply.github.com")).toBe(
      "Приложения установятся. При первом запуске iPhone попросит войти в Apple ID friend@users.noreply.github.com. Обновлений через App Store у них не будет — их там больше нет.",
    );
    const t = otherIdText(null);
    expect(t).toContain("войти в Apple ID, с которого скачивали.");
    expect(t).not.toContain(" ,");
  });
});

describe("canProceed", () => {
  const p = (target: boolean): PhoneDto => ({ udid: "u", name: "n", ios_version: "26", device_class: "iPhone", product_type: "p", target });
  it("needs a target device and an answer about the Apple ID", () => {
    expect(canProceed([p(true)], true)).toBe(true);
    expect(canProceed([p(true)], false)).toBe(true);
    expect(canProceed([p(true)], null)).toBe(false);
    expect(canProceed([p(false)], true)).toBe(false);
    expect(canProceed([], true)).toBe(false);
  });
});

const dev = (cls: string, target: boolean): PhoneDto => ({ udid: cls + target, name: "n", ios_version: "26", device_class: cls, product_type: "p", target });

describe("installsToIphone", () => {
  it("depends only on the target device", () => {
    expect(installsToIphone([dev("iPhone", true)])).toBe(true);
    expect(installsToIphone([dev("iPad", true), dev("iPhone", false)])).toBe(false);
    expect(installsToIphone([dev("iPhone", false)])).toBe(false);
    expect(installsToIphone([])).toBe(false);
  });
});

describe("showSkip", () => {
  it("«Продолжить без iPhone» only while there is no target", () => {
    expect(showSkip([])).toBe(true);
    expect(showSkip([dev("iPad", false)])).toBe(true);
    expect(showSkip([dev("iPhone", true)])).toBe(false);
  });
});

describe("askAppleIdOnDone", () => {
  it("skipped the phone, a phone appeared, not answered yet", () => {
    expect(askAppleIdOnDone(false, null, [dev("iPhone", true)])).toBe(true);
    expect(askAppleIdOnDone(false, null, [])).toBe(false);
    expect(askAppleIdOnDone(false, true, [dev("iPhone", true)])).toBe(false);
    expect(askAppleIdOnDone(true, null, [dev("iPhone", true)])).toBe(false);
  });
});

describe("accountBlock", () => {
  it("same Apple ID: no question", () => {
    expect(accountBlock({ kind: "same" }, "me@example.com")).toEqual({ kind: "none" });
  });
  it("different: warning with masked phone account and the program account", () => {
    expect(accountBlock({ kind: "different", phone_masked: "ow***@example.com" }, "me@example.com")).toEqual({
      kind: "different",
      text: "На этом iPhone покупки с ow***@example.com. Приложения с вашего Apple ID при запуске попросят войти в me@example.com.",
    });
    expect(accountBlock({ kind: "different", phone_masked: "ow***@example.com" }, null)).toEqual({
      kind: "different",
      text: "На этом iPhone покупки с ow***@example.com. Приложения с вашего Apple ID при запуске попросят войти в ваш Apple ID.",
    });
  });
  it("unknown or not checked yet: the old question", () => {
    expect(accountBlock({ kind: "unknown" }, "me@example.com")).toEqual({ kind: "question" });
    expect(accountBlock(null, "me@example.com")).toEqual({ kind: "question" });
  });
});

describe("sameFromMatch", () => {
  it("same answers the question, different and unknown leave it open", () => {
    expect(sameFromMatch({ kind: "same" })).toBe(true);
    expect(sameFromMatch({ kind: "different", phone_masked: "ow***@example.com" })).toBe(null);
    expect(sameFromMatch({ kind: "unknown" })).toBe(null);
  });
});

describe("afterLoginScreen", () => {
  it("after «Войти в Apple ID этого iPhone» straight to «Выбор» when the account now matches", () => {
    expect(afterLoginScreen(true, { kind: "same" })).toBe("choose");
    expect(afterLoginScreen(true, { kind: "different", phone_masked: "ot***@example.com" })).toBe("phone");
    expect(afterLoginScreen(true, { kind: "unknown" })).toBe("phone");
    expect(afterLoginScreen(true, null)).toBe("phone");
    expect(afterLoginScreen(false, { kind: "same" })).toBe("phone");
  });
});

describe("unknownAccountNote", () => {
  it("only for copies without a known Apple ID", () => {
    expect(unknownAccountNote({ unknown_account: true })).toBe("Неизвестный Apple ID");
    expect(unknownAccountNote({ unknown_account: false })).toBe(null);
  });
});
