import { describe, expect, it } from "vitest";
import { appsWord, returnLabel } from "./plural";

describe("appsWord", () => {
  it.each([
    [1, "приложение"],
    [2, "приложения"],
    [4, "приложения"],
    [5, "приложений"],
    [11, "приложений"],
    [12, "приложений"],
    [14, "приложений"],
    [21, "приложение"],
    [22, "приложения"],
    [25, "приложений"],
    [101, "приложение"],
    [111, "приложений"],
    [0, "приложений"],
  ])("%i → %s", (n, word) => {
    expect(appsWord(n)).toBe(word);
  });
});

describe("returnLabel", () => {
  it("names the action with the count", () => {
    expect(returnLabel(1)).toBe("Вернуть 1 приложение");
    expect(returnLabel(2)).toBe("Вернуть 2 приложения");
    expect(returnLabel(5)).toBe("Вернуть 5 приложений");
    expect(returnLabel(11)).toBe("Вернуть 11 приложений");
    expect(returnLabel(21)).toBe("Вернуть 21 приложение");
    expect(returnLabel(22)).toBe("Вернуть 22 приложения");
  });
  it("without a choice asks to choose", () => {
    expect(returnLabel(0)).toBe("Выберите приложения");
  });
});
