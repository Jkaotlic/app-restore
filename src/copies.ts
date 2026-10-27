// Экраны «Apple ID» и «Все копии»: чистые помощники (группы, подписи, кнопка).
import type { AccountMatchDto, CopyDto, PhoneDto, RowDto, UiError, UseAccountDto } from "./types";

export const UNKNOWN_ID = "Неизвестный Apple ID";

export interface CopyGroup {
  title: string;
  copies: CopyDto[];
}

const norm = (s: string) => s.toLocaleLowerCase("ru").trim();

/** Группы по покупателю (маской), неизвестный — последним; внутри по названию. Поиск — по названию и bundle id. */
export function groupCopies(copies: CopyDto[], query: string): CopyGroup[] {
  const q = norm(query);
  const hit = (c: CopyDto) => !q || norm(c.name).includes(q) || norm(c.bundle_id).includes(q);
  const groups = new Map<string | null, CopyDto[]>();
  for (const c of copies.filter(hit)) {
    const k = c.purchaser_masked;
    groups.set(k, [...(groups.get(k) ?? []), c]);
  }
  const byName = (a: CopyDto, b: CopyDto) => a.name.localeCompare(b.name, "ru");
  return [...groups.entries()]
    .sort(([a], [b]) => (a === null ? 1 : b === null ? -1 : a.localeCompare(b)))
    .map(([k, list]) => ({ title: k ?? UNKNOWN_ID, copies: list.sort(byName) }));
}

/** Предупреждение под выбранной копией: покупатель не тот, что на телефоне. */
export function copyWarning(c: CopyDto, m: AccountMatchDto | null): string | null {
  if (m?.kind !== "different" || !c.purchaser_masked) return null;
  return `При первом запуске iPhone попросит войти в ${c.purchaser_masked}: нужен пароль этого Apple ID`;
}

/** Кнопка «Поставить на {телефон}». Без телефона или копии — выключена; «только для iPad» на iPhone — тоже. */
export function copyInstallButton(c: CopyDto | null, target: PhoneDto | undefined): { text: string; disabled: boolean } {
  const text = `Поставить на ${target?.name || "iPhone"}`;
  const disabled = !c || !target || (c.ipad_only && target.device_class === "iPhone");
  return { text, disabled };
}

/** Телефон экрана «Все копии»: свой выбор (`chosen`), без него — цель главного потока.
 * Выбранный, но отключённый — никакой: молча на другой телефон не ставим. */
export function copyPhone(phones: PhoneDto[], chosen: string | null): PhoneDto | undefined {
  return chosen === null ? phones.find((p) => p.target) : phones.find((p) => p.udid === chosen);
}

/** Список телефонов для карточек «Все копии»: «Ставим сюда» — у своего выбора, не у главного. */
export function markCopyPhone(phones: PhoneDto[], chosen: string | null): PhoneDto[] {
  const own = copyPhone(phones, chosen)?.udid;
  return phones.map((p) => ({ ...p, target: p.udid === own }));
}

/** Строки под названием копии. */
export function copyNotes(c: CopyDto): string[] {
  const out: string[] = [];
  if (c.version) out.push(`Версия ${c.version}`);
  if (c.ipad_only) out.push("Только для iPad");
  return out;
}

/** Склонение «копия / копии / копий». */
export function copiesWord(n: number): string {
  const d10 = n % 10;
  const d100 = n % 100;
  if (d10 === 1 && d100 !== 11) return "копия";
  if (d10 >= 2 && d10 <= 4 && (d100 < 12 || d100 > 14)) return "копии";
  return "копий";
}

export function accountCopiesText(n: number): string {
  return n > 0 ? `${n} ${copiesWord(n)}` : "Копий нет";
}

export function accountStatus(a: { saved: boolean }): string {
  return a.saved ? "Вход сохранён" : "Нужен вход";
}

/** Номер шага в левой колонке: «Apple ID» — и список, и форма входа. Вне шагов — -1. */
export function stepIndex(screen: string): number {
  const steps: Record<string, number> = { accounts: 0, login: 0, phone: 1, choose: 2, done: 3 };
  return steps[screen] ?? -1;
}

/** Строка-заготовка для значка и job-ряда («Готово»): у копии нет строки «Выбора». */
export function copyRow(c: CopyDto): RowDto {
  return {
    track_id: c.track_id ?? 0,
    bundle_id: c.bundle_id,
    name: c.name,
    version: c.version,
    removed_from_ru: false,
    has_saved_copy: true,
    owned: null,
    source: "saved",
    letter: null,
    color: null,
    installed_version: null,
    available_version: null,
    install_state: "unknown",
    checked: false,
    ipad_only: c.ipad_only,
    unknown_account: c.purchaser_masked === null,
    aliases: [],
    category: null,
    size_bytes: null,
    other_account: false,
    purchaser_masked: null,
  };
}

/** «Все копии» в левой колонке — везде, кроме первого предупреждения: его не обойти. */
export function showCopiesLink(screen: string): boolean {
  return screen !== "warning";
}

/** Куда после `use_account`: вошли — «iPhone»; нужен вход — форма; не сохранили нынешний вход — остаёмся. */
export function afterUseAccount(res: UseAccountDto): "phone" | "login" | "stay" {
  if (res.error) return "stay";
  return res.restored && res.account ? "phone" : "login";
}

/** Подпись строки Apple ID, пока идёт смена входа; дольше 3 с — Apple отвечает медленно. */
export function pickStatusText(slow: boolean): string {
  return slow ? "Apple отвечает медленно, проверяем вход…" : "Входим…";
}

/** Отказ сменить вход (идёт скачивание, не сохранился нынешний вход): человек остаётся на месте. */
export function staysOnRefusal(e: UiError): boolean {
  return ["downloads_running", "save_session_failed", "keychain_waiting", "busy"].includes(e.code);
}

/** Снимок прежнего входа не сохранился (отказ или окно разрешения macOS): можно идти дальше без него. */
export function offersSkipSave(e: UiError | null): boolean {
  return !!e && (e.code === "save_session_failed" || e.code === "keychain_waiting");
}

/** Что значит «без сохранения» — честно: пароль не хранится, прежний вход потом заново. */
export function skipSaveNote(previous: string | null): string {
  const who = previous ?? "прежнего Apple ID";
  return `Можно продолжить без сохранения: для ${who} потом снова понадобятся пароль и код. Пароли программа не хранит.`;
}
