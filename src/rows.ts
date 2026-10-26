// Строки экрана «Выбор»: группы, поиск, проверка покупок. Чистые функции, без DOM.
import type { OwnedEvent, RowDto, UiError } from "./types";

export type OwnedState = { kind: "checking" } | { kind: "yes" } | { kind: "no" } | { kind: "error"; error: UiError };
export type OwnedMap = Map<number, OwnedState>;

/** После этих ошибок Rust прекращает `check_owned` (см. `stops_owned_check`). */
const STOPS_CHECK = new Set(["not_logged_in", "account_disabled", "apple_changed_login"]);

const isFirstGroup = (r: RowDto) => r.source === "removed" || r.source === "link";

function fold(s: string): string {
  return s.toLocaleLowerCase("ru").replace(/ё/g, "е").trim();
}

export function matchesQuery(name: string, query: string): boolean {
  const q = fold(query);
  return q === "" || fold(name).includes(q);
}

/** «Пропали из App Store» (каталог и добавленные по ссылке) и «Остальные покупки» с поиском. */
export function groupRows(rows: RowDto[], query: string): { removed: RowDto[]; other: RowDto[] } {
  return {
    removed: rows.filter(isFirstGroup),
    other: rows.filter((r) => !isFirstGroup(r) && matchesQuery(r.name, query)),
  };
}

/** Пришли покупки: группа «Пропали из App Store» остаётся как на экране (права там уже
 * проверены), остальное берётся из нового списка. */
export function mergePurchases(current: RowDto[], incoming: RowDto[]): RowDto[] {
  const first = current.filter(isFirstGroup);
  const ids = new Set(first.map((r) => r.track_id));
  const rest = incoming.filter((r) => !isFirstGroup(r) && !ids.has(r.track_id));
  for (const r of rest) ids.add(r.track_id);
  // копии, которых нет среди покупок, не пропадают с экрана (и их галочки тоже)
  const copies = current.filter((r) => r.source === "saved" && !ids.has(r.track_id));
  return [...first, ...rest, ...copies];
}

/** Итог проверки прав в строку: права, доступная версия и состояние на телефоне. */
export function applyOwnedRow(rows: RowDto[], ev: OwnedEvent): RowDto[] {
  if (ev.error) return rows;
  return rows.map((r) =>
    r.track_id === ev.track_id
      ? { ...r, owned: ev.owned, available_version: ev.available_version, install_state: ev.install_state, checked: ev.checked }
      : r,
  );
}

/** Строка по ссылке встаёт наверх; если такая уже есть, обновляется на месте. */
export function addLinkRow(rows: RowDto[], added: RowDto): RowDto[] {
  if (rows.some((r) => r.track_id === added.track_id)) {
    return rows.map((r) => (r.track_id === added.track_id ? { ...r, owned: added.owned ?? r.owned } : r));
  }
  return [added, ...rows];
}

function fromOwned(owned: boolean | null): OwnedState {
  if (owned === true) return { kind: "yes" };
  if (owned === false) return { kind: "no" };
  return { kind: "checking" };
}

export function initialOwned(rows: RowDto[]): OwnedMap {
  return new Map(rows.map((r) => [r.track_id, fromOwned(r.owned)]));
}

/** Записи каталога, которые ещё не проверены. Уже известные Rust помнит сам. */
export function idsToCheck(rows: RowDto[]): number[] {
  return rows.filter((r) => r.source === "removed" && r.owned === null).map((r) => r.track_id);
}

export function applyOwned(map: OwnedMap, ev: OwnedEvent): OwnedMap {
  const m = new Map(map);
  if (ev.error) {
    m.set(ev.track_id, { kind: "error", error: ev.error });
    if (STOPS_CHECK.has(ev.error.code)) {
      for (const [id, s] of m) if (s.kind === "checking") m.set(id, { kind: "error", error: ev.error });
    }
  } else {
    m.set(ev.track_id, fromOwned(ev.owned));
  }
  return m;
}

/** Новый `check_owned` отменяет прежний, поэтому с повторяемой строкой отправляем и все ждущие. */
export function recheckIds(map: OwnedMap, id: number): number[] {
  const waiting = [...map].filter(([k, s]) => k !== id && s.kind === "checking").map(([k]) => k);
  return [id, ...waiting];
}

export function ownedView(s: OwnedState): { text: string; spinner: boolean; recheck: boolean } {
  switch (s.kind) {
    case "checking":
      return { text: "Проверяем…", spinner: true, recheck: false };
    case "yes":
      return { text: "Есть в ваших покупках", spinner: false, recheck: false };
    case "no":
      return { text: "Не скачивалось с этого Apple ID", spinner: false, recheck: false };
    case "error":
      return { text: "Не удалось проверить", spinner: false, recheck: true };
  }
}

/** Запись каталога можно выбрать, если покупка подтверждена или копия уже лежит на компьютере. */
export function canSelect(r: RowDto, s: OwnedState | undefined, targetIphone = false): boolean {
  if (r.ipad_only && targetIphone) return false;
  if (r.source !== "removed") return true;
  return s?.kind === "yes" || r.has_saved_copy;
}

const PALETTE = ["#2F6F6A", "#3A5A98", "#8A4F9E", "#B5523B", "#3E8E5A", "#A07A1F", "#4F6D7A", "#9B3D5C"];

export function iconFor(r: RowDto): { letter: string; color: string } {
  if (r.letter && r.color) return { letter: r.letter, color: r.color };
  const ch = r.name.match(/[\p{L}\p{N}]/u)?.[0];
  let h = 0;
  for (const c of r.name) h = (h * 31 + c.codePointAt(0)!) >>> 0;
  return {
    letter: r.letter ?? (ch ? ch.toLocaleUpperCase("ru") : "?"),
    color: r.color ?? PALETTE[h % PALETTE.length],
  };
}
