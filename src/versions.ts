// «Версия под этот iPhone»: запустится ли приложение на iOS телефона и выбор другой версии.
// Чистые функции, без DOM. Сравнение — как `ios_compat` в Rust.
import type { OwnedState } from "./rows";
import type { FoundVersionDto, RowDto, VersionDto } from "./types";

function parts(v: string): number[] | null {
  const t = v.trim();
  if (!t) return null;
  const p = t.split(".");
  return p.every((x) => /^\d+$/.test(x)) ? p.map(Number) : null;
}

/** iOS `have` не старше `need`; недостающие части — нули («17» = «17.0»). `null` — не разобрать. */
export function iosRunsOn(need: string, have: string): boolean | null {
  const n = parts(need);
  const h = parts(have);
  if (!n || !h) return null;
  for (let i = 0; i < Math.max(n.length, h.length); i++) {
    const a = h[i] ?? 0;
    const b = n[i] ?? 0;
    if (a !== b) return a > b;
  }
  return true;
}

/** «Нужна iOS X, на iPhone Y» — только если обе версии известны и телефон старше. */
export function tooOldNote(minIos: string | null, phoneIos: string | null): string | null {
  if (!minIos || !phoneIos) return null;
  return iosRunsOn(minIos, phoneIos) === false ? `Нужна iOS ${minIos.trim()}, на iPhone ${phoneIos.trim()}` : null;
}

/** Третья строка несовместимой плитки: выбрать можно, на телефон не поставим — и куда идти дальше. */
export function downloadOnlyNote(phoneIos: string): string {
  return `Скачаем, но на этот iPhone не поставим — можно найти версию под iOS ${phoneIos.trim()}`;
}

/** «Вернуть»: несовместимое с телефоном только скачивается, остальное ставится. */
export function splitStart<T>(items: T[], tooOld: (x: T) => boolean): { install: T[]; downloadOnly: T[] } {
  return { install: items.filter((x) => !tooOld(x)), downloadOnly: items.filter(tooOld) };
}

/** Выбранная человеком версия строки: external id для скачивания и как её назвать. */
export interface ChosenVersion {
  id: string;
  display: string | null;
}

/** «12.15.0 · 18.03.2022»; без названия — номер версии у Apple. Не спросили (пауза после 429) —
 * «не проверено». */
export function versionLabel(v: VersionDto): string {
  if (!v.checked) return `${v.display ?? `Версия № ${v.id}`} (не проверено)`;
  const m = v.released ? /^(\d{4})-(\d{2})-(\d{2})$/.exec(v.released) : null;
  const date = m ? ` · ${m[3]}.${m[2]}.${m[1]}` : "";
  if (!v.display) return date ? `Версия № ${v.id}${date}` : `Версия № ${v.id} (Apple не назвал)`;
  return `${v.display}${date}`;
}

/** Для `start`: (trackId, external id) выбранных версий среди запускаемых строк. */
export function startVersions(ids: number[], chosen: Map<number, ChosenVersion>): [number, string][] {
  return ids.flatMap((id) => {
    const c = chosen.get(id);
    return c ? [[id, c.id] as [number, string]] : [];
  });
}

/** «Другая версия…»: строку скачиваем по trackId текущим входом и права подтверждены. */
export function canPickVersion(r: RowDto, s: OwnedState | undefined): boolean {
  if (r.track_id <= 0 || r.other_account || r.unknown_account || r.source === "saved") return false;
  if (r.source === "purchase" || r.source === "link") return true;
  return s?.kind === "yes";
}

/** «Проверить остальные»: с какого места снова спросить страницу (`before` для `app_versions`).
 * `versions` — от новой к старой; `null` — непроверенных нет. */
export function recheckFrom(versions: VersionDto[]): { before: string | null } | null {
  const i = versions.findIndex((v) => !v.checked);
  if (i < 0) return null;
  return { before: i === 0 ? null : versions[i - 1].id };
}

/** Пауза после 429: «можно снова через N с»; время вышло — `null`. */
export function waitText(seconds: number): string | null {
  return seconds > 0 ? `Apple просит подождать: можно снова через ${seconds} с` : null;
}

/** «Найти подходящую версию» прямо под плиткой: последняя не запустится на iOS телефона,
 * другую версию не выбирали, а скачать по номеру можно. */
export function findOffered(r: RowDto, s: OwnedState | undefined, phoneIos: string | null, chosen: ChosenVersion | undefined): boolean {
  return !chosen && canPickVersion(r, s) && tooOldNote(r.min_ios, phoneIos) !== null;
}

/** Итог «Найти подходящую» словами. */
export function foundMessage(f: FoundVersionDto, ios: string): string {
  if (f.error) return f.error.text;
  if (f.next_ios === null) return "На iPhone новейшая iOS — подойдёт последняя версия.";
  if (f.version) return `${f.version.display ?? `Версия № ${f.version.id}`} вышла до iOS ${f.next_ios} — должна запуститься на iOS ${ios}. Перед установкой проверим ещё раз.`;
  if (f.exhausted) return "Проверили 10 версий, подходящей не нашли — попробуйте «Ещё старше»";
  if (f.none_older) return `Все версии вышли после iOS ${f.next_ios}: на iOS ${ios}, скорее всего, не встанет ни одна.`;
  return "Не удалось найти: Apple не дал даты версий. Выберите версию из списка.";
}
