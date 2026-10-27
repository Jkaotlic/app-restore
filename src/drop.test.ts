import { describe, expect, it } from "vitest";
import { addedText, batchText, dropMessages, mergePending, MAX_PENDING, NO_FILES_TEXT, pendingNote, runBatch } from "./drop";
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

describe("batch of dropped ids", () => {
  it("adds one by one, collects unique errors", async () => {
    const seen: number[] = [];
    const steps: string[] = [];
    const r = await runBatch(
      [1, 2, 3, 4],
      async (id) => {
        seen.push(id);
        if (id % 2 === 0) throw new Error("нет прав");
      },
      { cancelled: () => false, step: (i, n) => steps.push(`${i}/${n}`), errorText: (e) => (e as Error).message },
    );
    expect(seen).toEqual([1, 2, 3, 4]);
    expect(steps).toEqual(["1/4", "2/4", "3/4", "4/4"]);
    expect(r).toEqual({ added: 2, total: 4, errors: ["нет прав"], cancelled: false });
    expect(batchText(r)).toBe("Добавлено 2 из 4. нет прав");
  });

  it("stops when cancelled — the rest is not asked", async () => {
    let stop = false;
    const seen: number[] = [];
    const r = await runBatch(
      [1, 2, 3],
      async (id) => {
        seen.push(id);
        stop = true;
      },
      { cancelled: () => stop, errorText: String },
    );
    expect(seen).toEqual([1]);
    expect(r).toEqual({ added: 1, total: 3, errors: [], cancelled: true });
    expect(batchText(r)).toBe("Добавлено 1 из 3 · остановлено");
  });
});
