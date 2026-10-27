// Строки экрана «Выбор»: группы, поиск, проверка покупок. Чистые функции, без DOM.
import type { OwnedEvent, PhoneDto, RowDto, SweepDto, UiError } from "./types";
import { DOWNLOAD_ONLY_NOTE, tooOldNote, type ChosenVersion } from "./versions";

/** `gone` — Apple больше не отдаёт приложение (`app_gone`): окончательный ответ, как «нет». */
export type OwnedState = { kind: "checking" } | { kind: "yes" } | { kind: "no" } | { kind: "gone" } | { kind: "error"; error: UiError };
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

/** Короче — в App Store не ищем, группы «В App Store» нет. */
const STORE_MIN = 3;

/** Запрос для `search_store`; `null` — короткий или пустой. */
export function storeTerm(query: string): string | null {
  const t = query.trim();
  return [...t].length >= STORE_MIN ? t : null;
}

/** Строка видна при этом запросе. Строка App Store — ответ на свой запрос (`store_term`): имя может
 * и не совпадать; ответ на прежний запрос скрыт (выбранная остаётся в «скрыто фильтром»). */
function shownFor(r: RowDto, query: string): boolean {
  if (r.source !== "store") return matchesRow(r, query);
  const t = storeTerm(query);
  return t !== null && (fold(r.store_term ?? "") === fold(t) || matchesRow(r, query));
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

/** Стрелки в панели чипов (roving tabindex): куда перевести фокус; `null` — клавиша не наша. */
export function rovingIndex(key: string, current: number, n: number): number | null {
  if (key === "ArrowRight") return (current + 1) % n;
  if (key === "ArrowLeft") return (current - 1 + n) % n;
  if (key === "Home") return 0;
  if (key === "End") return n - 1;
  return null;
}

export type GroupKey = "catalog" | "phone" | "buy" | "store";

function groupOf(r: RowDto): GroupKey {
  if (isFirstGroup(r)) return "catalog";
  if (r.source === "store") return "store";
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
  const found = rows.filter((r) => shownFor(r, query));
  const out = {} as Record<Chip, number>;
  for (const { key } of CHIPS) out[key] = found.filter((r) => inChip(r, key, withPhone)).length;
  return out;
}

const TITLES: Record<GroupKey, string> = { catalog: "Каталог", phone: "С этого iPhone", buy: "Мои покупки", store: "В App Store" };

/** Группы «Выбора» после чипа и поиска, всегда первые три и в этом порядке (пустые тоже);
 * «В App Store» — четвёртой, только при запросе от трёх букв. */
export function visibleGroups(rows: RowDto[], chip: Chip, query: string, withPhone = true): { key: GroupKey; title: string; rows: RowDto[] }[] {
  const shown = rows.filter((r) => inChip(r, chip, withPhone) && shownFor(r, query));
  const keys: GroupKey[] = storeTerm(query) === null ? ["catalog", "phone", "buy"] : ["catalog", "phone", "buy", "store"];
  return keys.map((key) => ({ key, title: TITLES[key], rows: shown.filter((r) => groupOf(r) === key) }));
}

/** Сколько выбранных строк не видно из-за чипа или поиска: «Снять всё» и «Вернуть» их тоже касаются. */
export function hiddenCount(chosen: RowDto[], chip: Chip, query: string, withPhone: boolean): number {
  return chosen.filter((r) => !(inChip(r, chip, withPhone) && shownFor(r, query))).length;
}

/** Надпись нижней панели без размера. */
export function selectionLabel(count: number, hidden: number): string {
  if (!count) return "Ничего не выбрано";
  return hidden ? `Выбрано ${count} · ${hidden} скрыто фильтром` : `Выбрано ${count}`;
}

/** Короткий итог для экранного диктора вместо озвучивания всего списка. */
export function foundText(rows: RowDto[], chip: Chip, query: string, withPhone = true): string {
  return `Найдено ${visibleGroups(rows, chip, query, withPhone).reduce((n, g) => n + g.rows.length, 0)}`;
}

/** «Выбрать все в группе»: что выбирается и выбрано ли уже всё. Уже стоящие той же версии пропускаются. */
export function groupPick(rows: RowDto[], selectable: (r: RowDto) => boolean, selected: Set<number>): { ids: number[]; allOn: boolean } {
  // уже стоит та же (или новее) версия — переставлять незачем, как и при отметке по умолчанию
  const ids = rows.filter((r) => selectable(r) && r.install_state !== "installed").map((r) => r.track_id);
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
  // копии и строки App Store, которых нет среди покупок, не пропадают с экрана (и их галочки тоже)
  const kept = current.filter((r) => (r.source === "saved" || r.source === "store") && !ids.has(r.track_id));
  return [...first, ...rest, ...kept];
}

/** Ответ App Store на запрос `term`: прежние невыбранные строки App Store уходят, выбранные остаются;
 * то, что уже есть на экране, второй раз не добавляется. */
export function replaceStoreRows(rows: RowDto[], incoming: RowDto[], selected: Set<number>, term: string): RowDto[] {
  const kept = dropStoreRows(rows, selected);
  const ids = new Set(kept.map((r) => r.track_id));
  const added = incoming.filter((r) => !ids.has(r.track_id) && ids.add(r.track_id)).map((r) => ({ ...r, store_term: term }));
  return [...kept, ...added];
}

/** Запрос стёрт или стал короче трёх букв: невыбранные строки App Store убрать. */
export function dropStoreRows(rows: RowDto[], selected: Set<number>): RowDto[] {
  return rows.filter((r) => r.source !== "store" || selected.has(r.track_id));
}

/** Отметка по умолчанию из `owned` (Rust `checked`): строку App Store человек отмечает сам. */
export function autoChecks(r: RowDto): boolean {
  return r.source !== "store";
}

/** Права проверяет `check_owned`: каталог, App Store и «С этого iPhone» (кроме чужого Apple ID). */
function needsCheck(r: RowDto): boolean {
  return r.source === "removed" || r.source === "store" || (r.source === "phone" && !r.other_account);
}

/** Подсказка у нижней панели, когда телефона нет. */
export const NO_PHONE_HINT = "Подключите iPhone, чтобы вернуть. Без него можно только скачать копии";

/** Нижняя панель: «Вернуть N» (нужен телефон) и «Только скачать N» (и без телефона). */
export function barState(count: number, hasPhone: boolean): { go: { text: string; disabled: boolean }; only: { text: string; disabled: boolean }; hint: string | null } {
  return {
    go: { text: count ? `Вернуть ${count}` : "Вернуть", disabled: count === 0 || !hasPhone },
    only: { text: count ? `Только скачать ${count}` : "Только скачать", disabled: count === 0 },
    hint: hasPhone ? null : NO_PHONE_HINT,
  };
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
  return rows.filter((r) => needsCheck(r) && r.owned === null).map((r) => r.track_id);
}

export function applyOwned(map: OwnedMap, ev: OwnedEvent): OwnedMap {
  const m = new Map(map);
  if (ev.error?.code === "app_gone") {
    m.set(ev.track_id, { kind: "gone" });
  } else if (ev.error) {
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
    .filter(needsCheck)
    .filter((r) => map.get(r.track_id)?.kind === "checking")
    .map((r) => r.track_id);
}

/** Видимые при поиске/фильтре строки, которые ещё проверяются (порядок экрана): Rust
 * (`owned_visible`) проверяет их первыми. */
export function visibleChecks(groups: { rows: RowDto[] }[], map: OwnedMap): number[] {
  return groups.flatMap((g) => g.rows).filter((r) => map.get(r.track_id)?.kind === "checking").map((r) => r.track_id);
}

/** Новый `check_owned` отменяет прежний, поэтому с повторяемой строкой отправляем и все ждущие. */
export function recheckIds(map: OwnedMap, id: number): number[] {
  const waiting = [...map].filter(([k, s]) => k !== id && s.kind === "checking").map(([k]) => k);
  return [id, ...waiting];
}

export type TileTone = "plain" | "muted" | "warn" | "checking" | "retry";

export type TileLine = { text: string; tone: TileTone; up?: string; note?: string };

/** Вторая строка плитки. `up` — новая версия, её выделяем: «на iPhone 21.3 → 22.1». `retry` — плитка
 * по нажатию повторяет проверку прав. `note` — третья строка рядом с версией («Неизвестный Apple ID», «Есть копия на компьютере»).
 * `phoneIos` — iOS телефона-цели: последней версии нужна новее — «Нужна iOS X, на iPhone Y» (скачать можно).
 * `chosen` — выбрана другая версия («Другая версия…»). */
export function tileLine(
  r: RowDto,
  s: OwnedState | undefined,
  toIphone: boolean,
  withPhone: boolean,
  opts: { phoneIos?: string | null; chosen?: ChosenVersion | null } = {},
): TileLine {
  if (r.ipad_only && toIphone) return { text: "только для iPad", tone: "warn" };
  if (r.other_account) return { text: r.purchaser_masked ? `Куплено на другом Apple ID (${r.purchaser_masked})` : "Куплено на другом Apple ID", tone: "warn" };
  if ((r.source === "removed" || r.source === "phone" || r.source === "store") && !r.has_saved_copy) {
    if (!s || s.kind === "checking") return { text: "проверяем…", tone: "checking" };
    if (s.kind === "gone") return { text: "Apple больше не отдаёт", tone: "muted" };
    if (s.kind === "no") return r.source === "store" ? { text: "Нет на вашем Apple ID", tone: "muted", note: NAMESAKE_NOTE } : { text: "Нет на вашем Apple ID", tone: "muted" };
    if (s.kind === "error") return { text: "Не удалось проверить · нажмите, чтобы повторить", tone: "retry" };
  }
  if (opts.chosen) return { text: opts.chosen.display ? `версия ${opts.chosen.display} (выбрана)` : "выбрана другая версия", tone: "plain" };
  const tooOld = toIphone ? tooOldNote(r.min_ios, opts.phoneIos ?? null) : null;
  if (tooOld) return { text: tooOld, tone: "warn", note: DOWNLOAD_ONLY_NOTE };
  const note = r.unknown_account ? "Неизвестный Apple ID" : r.has_saved_copy ? "Есть копия на компьютере" : undefined;
  const withNote = (l: TileLine): TileLine => (note ? { ...l, note } : l);
  if (withPhone && r.installed_version !== null) {
    const have = r.installed_version || "есть";
    if (hasNewer(r)) return withNote({ text: `на iPhone ${have} → `, up: r.available_version!, tone: "plain" });
    return withNote({ text: `на iPhone ${have}`, tone: "plain" });
  }
  // строка телефона-источника, которой нет на цели: версия со старого телефона
  if (r.source === "phone" && r.version) return withNote({ text: `с телефона ${r.version}`, tone: "plain" });
  if (r.available_version) return withNote({ text: `доступно ${r.available_version}`, tone: "plain" });
  if (r.unknown_account) return { text: "Неизвестный Apple ID", tone: "muted" };
  if (r.has_saved_copy) return { text: "Есть копия на компьютере", tone: "plain" };
  if (r.source === "purchase" && r.removed_from_ru) return { text: "нет в российском App Store", tone: "muted" };
  return { text: "в ваших покупках", tone: "muted" };
}

/** Состояние плитки для экранного диктора. Невыбираемая остаётся в порядке Tab (`aria-disabled`, а не
 * `disabled`) и называет причину; плитка «повторить проверку» — обычная кнопка, не переключатель. */
export function tileA11y(name: string, line: TileLine, enabled: boolean, isSelected: boolean): { pressed: "true" | "false" | null; disabled: boolean; label: string | null } {
  if (line.tone === "retry") return { pressed: null, disabled: false, label: null };
  if (!enabled) return { pressed: "false", disabled: true, label: `${name}, не выбрать: ${line.text}` };
  return { pressed: String(isSelected) as "true" | "false", disabled: false, label: null };
}

/** Запись каталога можно выбрать, если покупка подтверждена или копия уже лежит на компьютере.
 * Строку «С этого iPhone» — так же, и только если она куплена не на другом Apple ID. */
export function canSelect(r: RowDto, s: OwnedState | undefined, targetIphone = false): boolean {
  if (r.ipad_only && targetIphone) return false;
  if (r.other_account) return false;
  if (r.source !== "removed" && r.source !== "phone" && r.source !== "store") return true;
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

/** Третья строка плитки «В App Store» без прав: тёзка — не обязательно то самое приложение. */
export const NAMESAKE_NOTE = "Если у вас было приложение с таким названием, это могло быть другое — удалённое или скрытое";

/** Подсказка под группой «В App Store»: где вернуть скрытую покупку. */
export const HIDDEN_PURCHASES_HINT = "Скрытые покупки: App Store → аватар → ваше имя → Скрытые покупки → Показать";

/** Подсказка под группой «С этого iPhone». */
export const SOURCE_HINT = "Само приложение с телефона не забрать — копию скачаем с Apple по номеру. Работает, если приложение есть на Apple ID";

/** Подсказка на «Входе» после «Войти в этот Apple ID» у плитки, купленной на другом Apple ID. */
export const OTHER_ID_LOGIN_NOTE = "Войдите в Apple ID, с которого скачано это приложение";

/** Выбор «Телефон-источник» над группой: только при двух и больше подключённых. Цель помечена. */
export function sourceChoices(phones: PhoneDto[], source: string | null): { udid: string; label: string; selected: boolean }[] | null {
  if (phones.length < 2) return null;
  return phones.map((p) => ({ udid: p.udid, label: p.target ? `${p.name} (сюда ставим)` : p.name, selected: p.udid === source }));
}

/** Другой источник: строки группы «С этого iPhone» заменяются новыми, остальные остаются. */
export function replacePhoneRows(rows: RowDto[], incoming: RowDto[]): RowDto[] {
  const rest = rows.filter((r) => r.source !== "phone");
  const ids = new Set(rest.map((r) => r.track_id));
  return [...rest, ...incoming.filter((r) => !ids.has(r.track_id))];
}

/** Пришли покупки (`purchases`), помеченные источником `tag`. Человек уже выбрал другой (`chosen`,
 * `set_source_phone`) — строки группы на экране остаются, остальное берётся новое, а группу надо
 * перестроить (`stale`). Без явного выбора событие берётся как есть. */
export function purchasesForSource(current: RowDto[], incoming: RowDto[], tag: string | null, chosen: string | null): { rows: RowDto[]; stale: boolean } {
  if (chosen === null || tag === chosen) return { rows: incoming, stale: false };
  return { rows: replacePhoneRows(incoming, current.filter((r) => r.source === "phone")), stale: true };
}

/** «Проверено N из M» у «Проверить весь каталог»; `null` — не запускали. */
export function sweepText(p: SweepDto | null): string | null {
  if (!p) return null;
  // команда ещё не ответила, сколько всего
  if (p.running && p.total === 0) return "Проверяем…";
  if (!p.running && p.checked >= p.total) return "Весь каталог проверен";
  return `Проверено ${p.checked} из ${p.total}`;
}

/** Кнопка «Проверить весь каталог»: с входом, не во время проверки и пока есть что проверить. */
export function showSweepButton(p: SweepDto | null, loggedIn: boolean): boolean {
  return loggedIn && (!p || (!p.running && p.checked < p.total));
}

/** Бейдж «Новое»: запись каталога, о выпуске которой сказал Rust (`release_badges`). */
export function isNewRow(r: RowDto, newIds: Set<number>): boolean {
  return r.source === "removed" && newIds.has(r.track_id);
}

/** Перед проверкой всего каталога: его строки с ошибкой снова «проверяем…». */
export function retryCatalogErrors(rows: RowDto[], map: OwnedMap): OwnedMap {
  const m = new Map(map);
  for (const r of rows) if (r.source === "removed" && m.get(r.track_id)?.kind === "error") m.set(r.track_id, { kind: "checking" });
  return m;
}
