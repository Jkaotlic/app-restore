import { describe, expect, it } from "vitest";
import { addedText, dropMessages, mergePending, MAX_PENDING, NO_FILES_TEXT, pendingNote } from "./drop";
import type { DropDto, UiError } from "./types";

const err = (code: string, text: string): UiError => ({ code, text, retryable: false });
const dto = (d: Partial<DropDto>): DropDto => ({ copies: [], ids: [], errors: [], no_files: false, ...d });

describe("drop", () => {
  it("messages: unique error texts, and a hint when no file came", () => {
    expect(dropMessages(dto({}))).toEqual([]);
    expect(dropMessages(dto({ errors: [err("not_ipa", "Это не приложение"), err("not_ipa", "Это не приложение"), err("bad_link", "Не ссылка")] }))).toEqual([
      "Это не приложение",
      "Не ссылка",
    ]);
    expect(dropMessages(dto({ no_files: true }))).toEqual([NO_FILES_TEXT]);
    expect(NO_FILES_TEXT).toContain("Добавить по ссылке");
  });

  it("pending links keep order, no repeats, capped", () => {
    expect(mergePending([3, 1], [1, 2, 2])).toEqual([3, 1, 2]);
    const many = Array.from({ length: 300 }, (_, i) => 1_000_000 + i);
    expect(mergePending([], many)).toHaveLength(MAX_PENDING);
  });

  it("texts", () => {
    expect(addedText(3, 3)).toBe("Добавлено: 3");
    expect(addedText(1, 3)).toBe("Добавлено 1 из 3");
    expect(addedText(0, 2)).toBe("Не добавлено ни одного из 2");
    expect(pendingNote(0)).toBeNull();
    expect(pendingNote(2)).toBe("Ссылок перетащено: 2 — добавятся на шаге «Выбор»");
  });
});
