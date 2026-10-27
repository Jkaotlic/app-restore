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

/** Поиск по имени и по любому другому названию записи каталога. */
export function matchesRow(r: RowDto, query: string): boolean {
  return matchesQuery(r.name, query) || r.aliases.some((a) => fold(a).includes(fold(query)));
}

/** Есть версия новее: обе версии известны, разные, и на телефоне не новее и не та же (Rust). */
export function hasNewer(r: RowDto): boolean {
  return r.installed_version !== null && !!r.available_version && r.installed_version !== r.available_version && r.install_state !== "installed";
}

export type Chip = "all" | "bank" | "social" | "travel" | "buy" | "on" | "new";

const CHIPS: { key: Chip; label: string }[] = [
  { key: "all", label: "Все" },
  { key: "bank", label: "Банки" },
  { key: "social", label: "Соцсети и почта" },
  { key: "travel", label: "Транспорт и покупки" },
  { key: "buy", label: "Мои покупки" },
  { key: "on", label: "На iPhone" },
  { key: "new", label: "Есть новее" },
];

/** Без телефона «На iPhone» и «Есть новее» всегда пусты — их не показываем. */
export function chipList(withPhone: boolean): { key: Chip; label: string }[] {
  return withPhone ? CHIPS : CHIPS.filter((c) => c.key !== "on" && c.key !== "new");
}

export type GroupKey = "catalog" | "phone" | "buy";

function groupOf(r: RowDto): GroupKey {
  if (isFirstGroup(r)) return "catalog";
  return r.source === "phone" ? "phone" : "buy";
}

function inChip(r: RowDto, chip: Chip, withPhone: boolean): boolean {
  switch (chip) {
    case "all":
      return true;
    case "bank":
    case "social":
    case "travel":
      return r.category === chip;
    case "buy":
      return groupOf(r) === "buy";
    case "on":
      return withPhone && r.installed_version !== null;
    case "new":
      return withPhone && hasNewer(r);
  }
}

/** Счётчики на чипах: каждый с учётом поиска. */
export function chipCounts(rows: RowDto[], query: string, withPhone: boolean): Record<Chip, number> {
  const found = rows.filter((r) => matchesRow(r, query));
  const out = {} as Record<Chip, number>;
  for (const { key } of CHIPS) out[key] = found.filter((r) => inChip(r, key, withPhone)).length;
  return out;
}

const TITLES: Record<GroupKey, string> = { catalog: "Каталог", phone: "С этого iPhone", buy: "Мои покупки" };

/** Группы «Выбора» после чипа и поиска, всегда все три и в этом порядке (пустые тоже). */
export function visibleGroups(rows: RowDto[], chip: Chip, query: string, withPhone = true): { key: GroupKey; title: string; rows: RowDto[] }[] {
  const shown = rows.filter((r) => inChip(r, chip, withPhone) && matchesRow(r, query));
  return (["catalog", "phone", "buy"] as const).map((key) => ({ key, title: TITLES[key], rows: shown.filter((r) => groupOf(r) === key) }));
}

/** «Выбрать все в группе»: что выбирается и выбрано ли уже всё. */
export function groupPick(rows: RowDto[], selectable: (r: RowDto) => boolean, selected: Set<number>): { ids: number[]; allOn: boolean } {
  const ids = rows.filter(selectable).map((r) => r.track_id);
  return { ids, allOn: ids.length > 0 && ids.every((id) => selected.has(id)) };
}

export function emptySearchText(query: string): string {
  return `Ничего не нашлось по «${query.trim()}». Попробуйте другое название или вставьте ссылку из App Store ниже.`;
}

const MB = 1024 * 1024;
const GB = 1024 * MB;

/** «Выбрано N · ≈ размер»: сумма известных размеров; часть неизвестна — «+»; ни одного — без размера. */
export function selectionSummary(rows: RowDto[]): { count: number; size: string | null } {
  const known = rows.filter((r) => r.size_bytes !== null);
  if (!known.length) return { count: rows.length, size: null };
  const bytes = known.reduce((s, r) => s + (r.size_bytes ?? 0), 0);
  const text = bytes >= GB ? `${(bytes / GB).toFixed(1).replace(".", ",")} ГБ` : `${Math.max(1, Math.round(bytes / MB))} МБ`;
  return { count: rows.length, size: `≈ ${text}${known.length < rows.length ? "+" : ""}` };
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

/** Записи каталога и строки «С этого iPhone» (кроме чужого Apple ID), которые ещё не проверены.
 * Уже известные Rust помнит сам. */
export function idsToCheck(rows: RowDto[]): number[] {
  return rows
    .filter((r) => (r.source === "removed" || (r.source === "phone" && !r.other_account)) && r.owned === null)
    .map((r) => r.track_id);
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

/** Что ещё ждёт `check_owned`: новый вызов отменяет прежний, поэтому после покупок шлём всё ждущее
 * (каталог и строки «С этого iPhone»). */
export function pendingChecks(rows: RowDto[], map: OwnedMap): number[] {
  return rows
    .filter((r) => r.source === "removed" || (r.source === "phone" && !r.other_account))
    .filter((r) => map.get(r.track_id)?.kind === "checking")
    .map((r) => r.track_id);
}

/** Новый `check_owned` отменяет прежний, поэтому с повторяемой строкой отправляем и все ждущие. */
export function recheckIds(map: OwnedMap, id: number): number[] {
  const waiting = [...map].filter(([k, s]) => k !== id && s.kind === "checking").map(([k]) => k);
  return [id, ...waiting];
}

export type TileTone = "plain" | "muted" | "warn" | "checking" | "retry";

/** Вторая строка плитки. `up` — новая версия, её выделяем: «на iPhone 21.3 → 22.1». `retry` — плитка
 * по нажатию повторяет проверку прав. */
export function tileLine(r: RowDto, s: OwnedState | undefined, toIphone: boolean, withPhone: boolean): { text: string; tone: TileTone; up?: string } {
  if (r.ipad_only && toIphone) return { text: "только для iPad", tone: "warn" };
  if (r.other_account) return { text: "Куплено на другом Apple ID", tone: "warn" };
  if ((r.source === "removed" || r.source === "phone") && !r.has_saved_copy) {
    if (!s || s.kind === "checking") return { text: "проверяем…", tone: "checking" };
    if (s.kind === "no") return { text: "Нет на вашем Apple ID", tone: "muted" };
    if (s.kind === "error") return { text: "Не удалось проверить · нажмите, чтобы повторить", tone: "retry" };
  }
  if (withPhone && r.installed_version !== null) {
    const have = r.installed_version || "есть";
    if (hasNewer(r)) return { text: `на iPhone ${have} → `, up: r.available_version!, tone: "plain" };
    return { text: `на iPhone ${have}`, tone: "plain" };
  }
  if (r.available_version) return { text: `доступно ${r.available_version}`, tone: "plain" };
  if (r.unknown_account) return { text: "Неизвестный Apple ID", tone: "muted" };
  if (r.has_saved_copy) return { text: "копия на компьютере", tone: "plain" };
  if (r.source === "purchase" && r.removed_from_ru) return { text: "нет в российском App Store", tone: "muted" };
  return { text: "в ваших покупках", tone: "muted" };
}

/** Запись каталога можно выбрать, если покупка подтверждена или копия уже лежит на компьютере.
 * Строку «С этого iPhone» — так же, и только если она куплена не на другом Apple ID. */
export function canSelect(r: RowDto, s: OwnedState | undefined, targetIphone = false): boolean {
  if (r.ipad_only && targetIphone) return false;
  if (r.other_account) return false;
  if (r.source !== "removed" && r.source !== "phone") return true;
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

/** Светлый фон значка (жёлтый Т-Банка): букву тогда пишем тёмной. Яркость по WCAG. */
export function isLight(color: string): boolean {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color);
  if (!m) return false;
  const lin = (x: string) => {
    const c = parseInt(x, 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(m[1]) + 0.7152 * lin(m[2]) + 0.0722 * lin(m[3]) > 0.45;
}
