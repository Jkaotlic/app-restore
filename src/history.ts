// «Найти всё, что вы скачивали» (история покупок Apple): тексты и слияние строк. Без DOM.
import type { RowDto } from "./types";

/** Подсказка под группой «Из истории покупок». */
export const HISTORY_HINT = "Скрыто или пропало из списка покупок. Скачать можно, если Apple ещё отдаёт приложение";

function apps(n: number): string {
  const d = n % 10;
  const dd = n % 100;
  if (d === 1 && dd !== 11) return "приложение";
  if (d >= 2 && d <= 4 && (dd < 12 || dd > 14)) return "приложения";
  return "приложений";
}

/** Что будет, до открытия окна Apple: это последовательность, поэтому с номерами. */
export const HISTORY_STEPS = [
  "Войдите своим Apple ID на странице Apple",
  "Пролистайте список покупок вниз до самого конца — так Apple подгружает все приложения",
  "Окно закроется само, найденное появится здесь",
];

/** Пока окно открыто: до первой пачки ждём входа. */
export function historyProgressText(found: number): string {
  return found ? `Найдено ${found} ${apps(found)}…` : "Окно Apple открыто — войдите своим Apple ID…";
}

/** Итог без ошибки: всего в истории и сколько из них новых для «Выбора». */
export function historyResultText(found: number, fresh: number): string {
  if (!found) return "Готово: в истории покупок приложений для iPhone не нашлось";
  if (!fresh) return `Готово: в истории ${found} ${apps(found)} — все уже есть в списке`;
  return `Готово: в истории ${found} ${apps(found)}, новых ${fresh} — в группе «Из истории покупок»`;
}

/** Новые строки истории в конец; то, что уже на экране, второй раз не добавляется. */
export function mergeHistoryRows(rows: RowDto[], incoming: RowDto[]): RowDto[] {
  const ids = new Set(rows.map((r) => r.track_id));
  return [...rows, ...incoming.filter((r) => !ids.has(r.track_id) && ids.add(r.track_id))];
}
