// Перетаскивание в окно: что сказать человеку и как копить номера до «Выбора». Чистые функции.
// Файлы разбирает Rust (`dropped.rs`), сюда приходит итог — событие `dropped`.
import type { DropDto } from "./types";

/** Tauri приносит только пути файлов: текст или ссылка из браузера сюда не доходят. */
export const NO_FILES_TEXT = "Сюда можно перетащить файл .ipa или текстовый файл со ссылками App Store. Ссылку из браузера вставьте в поле «Добавить по ссылке»";

/** Столько же номеров берёт Rust за одно перетаскивание (`app_link::MAX_TEXT_IDS`). */
export const MAX_PENDING = 100;

/** Тексты ошибок без повторов; перетащили не файл — подсказка. */
export function dropMessages(d: DropDto): string[] {
  const out = [...new Set(d.errors.map((e) => e.text))];
  if (d.no_files) out.push(NO_FILES_TEXT);
  return out;
}

/** Номера, что ждут «Выбора»: по порядку, без повторов, не больше `MAX_PENDING`. */
export function mergePending(pending: number[], ids: number[]): number[] {
  return [...new Set([...pending, ...ids])].slice(0, MAX_PENDING);
}

/** Итог добавления нескольких ссылок. */
export function addedText(added: number, total: number): string {
  if (added === 0) return `Не добавлено ни одного из ${total}`;
  return added === total ? `Добавлено: ${added}` : `Добавлено ${added} из ${total}`;
}

/** Подсказка в боковой панели, пока номера ждут «Выбора»; `null` — ждать нечего. */
export function pendingNote(n: number): string | null {
  return n ? `Ссылок перетащено: ${n} — добавятся на шаге «Выбор»` : null;
}

/** Итог пачки номеров (`runBatch`). */
export interface BatchResult {
  added: number;
  total: number;
  errors: string[];
  /** Остановлено («Отменить» или уход с «Выбора»): остальные номера не спрашивали. */
  cancelled: boolean;
}

/** Номера по одному через `add` (на «Выборе» — `add_by_link` с проверкой владения). Перед каждым
 * проверяется `cancelled`; `step(i, n)` — для «Проверяем i из n…». */
export async function runBatch(
  ids: number[],
  add: (id: number) => Promise<void>,
  opts: { cancelled: () => boolean; step?: (i: number, n: number) => void; errorText: (e: unknown) => string },
): Promise<BatchResult> {
  let added = 0;
  const errors = new Set<string>();
  for (const [i, id] of ids.entries()) {
    if (opts.cancelled()) return { added, total: ids.length, errors: [...errors], cancelled: true };
    opts.step?.(i + 1, ids.length);
    try {
      await add(id);
      added++;
    } catch (e) {
      errors.add(opts.errorText(e));
    }
  }
  return { added, total: ids.length, errors: [...errors], cancelled: false };
}

/** Текст итога пачки. */
export function batchText(r: BatchResult): string {
  const head = r.cancelled ? `${addedText(r.added, r.total)} · остановлено` : addedText(r.added, r.total);
  return [head, ...r.errors].join(". ");
}
