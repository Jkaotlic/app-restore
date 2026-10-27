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
