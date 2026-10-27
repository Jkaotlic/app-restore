import type { AccountMatchDto, JobDto, PhoneDto, PurchasesProgress, UiError } from "./types";

export interface JobView {
  text: string;
  /** null — без полосы; число — проценты; "indeterminate" — идёт, сколько осталось неизвестно. */
  bar: number | "indeterminate" | null;
  retry: boolean;
}

const MB = 1024 * 1024;

/** Строка задания на экране «iPhone». `downloadOnly` — «Только скачать»: скачанное и есть итог. */
export function jobView(s: JobDto, fromSaved: string | null, downloadOnly = false): JobView {
  switch (s.kind) {
    case "queued":
      return { text: "В очереди", bar: 0, retry: false };
    case "downloading":
      return { text: `Скачано ${Math.round(s.bytes / MB)} МБ`, bar: "indeterminate", retry: false };
    case "saved":
      return { text: downloadOnly ? "Сохранено" : "Скачано, ждём iPhone", bar: null, retry: false };
    case "installing":
      return { text: `Устанавливается, ${s.percent}%`, bar: s.percent, retry: false };
    case "installed":
      return { text: fromSaved ? `Поставлена сохранённая копия ${fromSaved}` : "Установлено", bar: null, retry: false };
    case "failed":
      return { text: errorText(s.error), bar: null, retry: true };
  }
}

/** Дальше задание само не сдвинется: установлено, ошибка или сохранено по «Только скачать». */
export function isFinished(s: JobDto, downloadOnly = false): boolean {
  return s.kind === "installed" || s.kind === "failed" || (downloadOnly && s.kind === "saved");
}

/** Строка на экране «Готово». С другим Apple ID говорим, в какой ID войти при первом запуске. */
export function doneText(s: JobDto, fromSaved: string | null, sameAppleId: boolean | null, email: string | null, downloadOnly = false): string {
  const base = jobView(s, fromSaved, downloadOnly).text;
  if (s.kind !== "installed" || sameAppleId !== false) return base;
  return `${base}. При первом запуске войдите в ${email ?? "Apple ID, с которого скачивали"}`;
}

export function phoneView(p: PhoneDto): { title: string; subtitle: string; isIpad: boolean } {
  const isIpad = p.device_class === "iPad";
  const os = p.ios_version ? `${isIpad ? "iPadOS" : "iOS"} ${p.ios_version}` : "";
  const head = isIpad ? "iPad" : "Подключён";
  return { title: p.name, subtitle: os ? `${head}, ${os}` : head, isIpad };
}

export function purchasesProgressText(p: PurchasesProgress | null): string {
  return p ? `Загружаем покупки: страница ${p.page} из ${p.pages}` : "Загружаем покупки";
}

/** Подсказка над выбором устройства; null — выбирать нечего или уже выбрано. */
export function chooserNote(phones: PhoneDto[]): string | null {
  if (phones.length > 1) return "Подключено несколько устройств. Выберите, куда ставить.";
  if (phones.length === 1 && !phones[0].target) {
    return phones[0].device_class === "iPad" ? "Это iPad. Подключите iPhone или выберите iPad" : "Выберите, куда ставить.";
  }
  return null;
}

/** Ответ «Нет, другой» на вопрос про Apple ID. */
export function otherIdText(email: string | null): string {
  const id = email ? `Apple ID ${email}` : "Apple ID, с которого скачивали";
  return `Приложения установятся. При первом запуске iPhone попросит войти в ${id}. Обновлений через App Store у них не будет — их там больше нет.`;
}

/** «Дальше» на шаге iPhone: устройство выбрано и на вопрос ответили. */
export function canProceed(phones: PhoneDto[], sameAppleId: boolean | null): boolean {
  return sameAppleId !== null && phones.some((p) => p.target);
}

/** Ставим на iPhone: решает только выбранное устройство, а не то, как прошли шаг iPhone. */
export function installsToIphone(phones: PhoneDto[]): boolean {
  return phones.find((p) => p.target)?.device_class === "iPhone";
}

/** «Продолжить без iPhone» — только пока устройство не выбрано; с ним идут через «Дальше». */
export function showSkip(phones: PhoneDto[]): boolean {
  return !phones.some((p) => p.target);
}

/** На «Готово» спросить про Apple ID: шаг iPhone пропустили, телефон появился, ответа нет. */
export function askAppleIdOnDone(withPhone: boolean, sameAppleId: boolean | null, phones: PhoneDto[]): boolean {
  return !withPhone && sameAppleId === null && phones.some((p) => p.target);
}

/** Что показать на шаге iPhone про Apple ID: ничего, прежний вопрос или предупреждение. */
export type AccountBlock = { kind: "none" } | { kind: "question" } | { kind: "different"; text: string };

/** `m` — итог `phone_account`; null — ещё не проверяли. `program` — Apple ID в программе. */
export function accountBlock(m: AccountMatchDto | null, program: string | null): AccountBlock {
  if (m?.kind === "same") return { kind: "none" };
  if (m?.kind === "different") {
    const id = program ?? "ваш Apple ID";
    return {
      kind: "different",
      text: `На этом iPhone покупки с ${m.phone_masked}. Приложения с вашего Apple ID при запуске попросят войти в ${id}.`,
    };
  }
  return { kind: "question" };
}

/** Ответ на «тот же Apple ID?» по итогу проверки: известен только при совпадении. */
export function sameFromMatch(m: AccountMatchDto): boolean | null {
  return m.kind === "same" ? true : null;
}

/** Куда после входа. После «Войти в Apple ID этого iPhone» — сразу на «Выбор», если аккаунт совпал. */
export function afterLoginScreen(switched: boolean, m: AccountMatchDto | null): "choose" | "phone" {
  return switched && m?.kind === "same" ? "choose" : "phone";
}

/** Текст ошибки для человека; у неизвестной — код строки журнала и где его открыть. */
export function errorText(e: UiError): string {
  return e.ref_code ? `${e.text} Код ${e.ref_code} — подробности в журнале («Открыть журнал» внизу слева).` : e.text;
}

/** Через сколько секунд «Входим…» показать, что Apple бывает медленным. */
export const LOGIN_SLOW_AFTER_S = 10;

/** Подпись под «Входим…», когда ждём дольше `LOGIN_SLOW_AFTER_S`; раньше — ничего. */
export function loginWaitNote(seconds: number): string | null {
  return seconds >= LOGIN_SLOW_AFTER_S ? "Apple иногда долго отвечает при первом входе — подождите до двух–трёх минут." : null;
}

/** Подсказка у поля кода 2FA. */
export const CODE_HINT =
  "Код приходит на iPhone или Mac с этим Apple ID, иногда с задержкой. Нет кода — на iPhone: Настройки → ваше имя → Вход и безопасность → Получить код проверки.";

/** Подпись под «Входим…»: подсказка про Связку ключей главнее; без неё — «Apple долго отвечает»
 * после `LOGIN_SLOW_AFTER_S`. Убранная подсказка не стирает уже показанную «медленную». */
export function loginNote(keychain: string | null, seconds: number): string | null {
  return keychain ?? loginWaitNote(seconds);
}
