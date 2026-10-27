import { describe, expect, it } from "vitest";
import { HELP_STEPS } from "./help";

describe("«Не нашли приложение?»", () => {
  it("all seven steps, in order, with their jump buttons", () => {
    expect(HELP_STEPS.map((s) => s.title)).toEqual([
      "История покупок Apple",
      "Скрытые покупки",
      "Семейный доступ",
      "«Сгружать неиспользуемые»",
      "Добавить по ссылке",
      "Подключить старый телефон",
      "Сообщить в каталог",
    ]);
    expect(HELP_STEPS.map((s) => s.action?.kind ?? null)).toEqual(["history", "refresh", "accounts", null, "link", "phone", null]);
    expect(HELP_STEPS[1].text).toContain("Скрытые покупки → Показать");
    expect(HELP_STEPS[1].action?.label).toBe("Обновить покупки");
    for (const s of HELP_STEPS) expect(s.text.length).toBeGreaterThan(20);
    // iOS 18+: Настройки → Приложения → App Store; старый путь — тоже
    expect(HELP_STEPS[3].text).toContain("Настройки → Приложения → App Store");
    expect(HELP_STEPS[3].text).toContain("Настройки → App Store");
    // не обещаем, что добавим; нужен подключённый телефон
    expect(HELP_STEPS[6].text).not.toMatch(/добавим/);
    expect(HELP_STEPS[6].text).toContain("чтобы его добавили в каталог");
    expect(HELP_STEPS[6].text).toContain("Подключите");
  });
});
