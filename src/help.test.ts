import { describe, expect, it } from "vitest";
import { HELP_STEPS, HISTORY_NOTE, LINK_NOTE } from "./help";

describe("«Не нашли приложение?»", () => {
  // история покупок и поле ссылки — свои строки блока, здесь только «Ещё способы»
  it("the other five ways, in order, with their jump buttons", () => {
    expect(HELP_STEPS.map((s) => s.title)).toEqual([
      "Скрытые покупки",
      "Семейный доступ",
      "«Сгружать неиспользуемые»",
      "Подключить старый телефон",
      "Сообщить в каталог",
    ]);
    expect(HELP_STEPS.map((s) => s.action?.kind ?? null)).toEqual(["refresh", "accounts", null, "phone", null]);
    expect(HELP_STEPS[0].text).toContain("Скрытые покупки → Показать");
    expect(HELP_STEPS[0].action?.label).toBe("Обновить покупки");
    for (const s of HELP_STEPS) expect(s.text.length).toBeGreaterThan(20);
    // iOS 18+: Настройки → Приложения → App Store; старый путь — тоже
    expect(HELP_STEPS[2].text).toContain("Настройки → Приложения → App Store");
    expect(HELP_STEPS[2].text).toContain("Настройки → App Store");
    // не обещаем, что добавим; нужен подключённый телефон
    expect(HELP_STEPS[4].text).not.toMatch(/добавим/);
    expect(HELP_STEPS[4].text).toContain("чтобы его добавили в каталог");
    expect(HELP_STEPS[4].text).toContain("Подключите");
  });

  it("history and link are not tips any more", () => {
    expect(LINK_NOTE).toContain("перетащите в окно");
    expect(HISTORY_NOTE).toContain("скрытое");
  });
});
