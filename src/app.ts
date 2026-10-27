// Общее состояние программы и переходы между экранами. Экраны сами строят свой DOM
// и получают уведомления о событиях Rust через `notify`.
import { invoke } from "@tauri-apps/api/core";
import { h } from "./dom";
import { showCopiesLink, stepIndex } from "./copies";
import { errorText, installsToIphone } from "./labels";
import type { AccountEntry, AccountMatchDto, Account, JobDto, OwnedEvent, PhoneDto, PurchasesProgress, RowDto, RowsDto, RunDto, UiError } from "./types";

export type Screen = "warning" | "accounts" | "login" | "choose" | "phone" | "done" | "copies";

export interface Job {
  track_id: number;
  name: string;
  row: RowDto;
  state: JobDto;
  fromSaved: string | null;
}

export interface State {
  screen: Screen;
  account: Account | null;
  /** Шаг iPhone пройден с телефоном («Дальше»); false — «Продолжить без iPhone». */
  withPhone: boolean;
  /** Галочки «Выбора» при уходе на шаг iPhone («Назад»); null — начать заново. */
  choice: { selected: number[]; unchecked: number[]; picked: number[] } | null;
  jobs: Map<number, Job>;
  /** Ответ на «тот же Apple ID?»; null — ещё не ответили. */
  sameAppleId: boolean | null;
  run: RunDto | null;
  /** Итог `phone_account` для телефона `udid`; null — не проверяли (или сменился вход). */
  accountMatch: { udid: string; m: AccountMatchDto } | null;
  /** Адрес для поля входа после «Войти в Apple ID этого iPhone». Не логируется. */
  loginPrefill: string | null;
  /** Вход идёт после «Войти в Apple ID этого iPhone»: потом сразу на «Выбор». */
  switchedAccount: boolean;
  /** «Добавить Apple ID»: поле адреса пустое, а не нынешний вход. */
  addingAccount: boolean;
  /** Подсказка на «Входе» один раз (например, «Войти в этот Apple ID» с «Выбора»). */
  loginNote: string | null;
  /** Откуда пришли на «Все копии»: туда ведёт «Назад». */
  beforeCopies: Screen;
  phones: PhoneDto[];
  phonesError: UiError | null;
  update: string | null;
  /** macOS ждёт разрешения Связки ключей (`keychain_note`): подсказка в боковой панели. */
  keychainNote: string | null;
}

let markReady: () => void = () => undefined;
/** Выполнен после `session_info` при запуске: известно, вошёл ли человек. */
export const sessionReady = new Promise<void>((r) => (markReady = r));
export function sessionChecked(): void {
  markReady();
}

export const state: State = {
  screen: "warning",
  account: null,
  withPhone: false,
  choice: null,
  jobs: new Map(),
  sameAppleId: null,
  run: null,
  accountMatch: null,
  loginPrefill: null,
  switchedAccount: false,
  addingAccount: false,
  loginNote: null,
  beforeCopies: "accounts",
  phones: [],
  phonesError: null,
  update: null,
  keychainNote: null,
};

/** Что экран хочет знать о событиях, пока он на виду. */
export interface Mounted {
  el: HTMLElement;
  onJob?(): void;
  onOwned?(ev: OwnedEvent): void;
  onPhones?(): void;
  onPurchasesProgress?(p: PurchasesProgress): void;
  onPurchases?(rows: RowsDto): void;
  /** Вызывается при уходе с экрана. */
  leave?(): void;
}

type ScreenFn = () => Mounted;
const screens = new Map<Screen, ScreenFn>();
let current: Mounted | null = null;

export function register(name: Screen, fn: ScreenFn): void {
  screens.set(name, fn);
}

export function currentScreen(): Mounted | null {
  return current;
}

const STEPS = ["Apple ID", "iPhone", "Выбор", "Готово"];

/** Устройство, на которое пойдёт установка. */
export function targetPhone(): PhoneDto | undefined {
  return state.phones.find((p) => p.target);
}

/** Ставим на iPhone: строки «только для iPad» не выбрать. Без телефона и для iPad — можно. */
export function targetIsIphone(): boolean {
  return installsToIphone(state.phones);
}

/** Обработчик клика для async-функции: ошибка не теряется молча. */
export function safe<E extends Event>(fn: (e: E) => Promise<unknown>): (e: E) => void {
  return (e) => void fn(e).catch((err) => console.error(err));
}

/** Кнопка «Открыть папку с копиями» и место для ошибки под ней. */
export function openLibraryButton(cls = "link"): HTMLElement {
  return openFolderButton("open_library", "Открыть папку с копиями", cls);
}

/** «Открыть журнал»: папка журнала для поддержки. Есть всегда, и без входа. */
export function openLogButton(cls = "link"): HTMLElement {
  return openFolderButton("open_log", "Открыть журнал", cls);
}

/** Кнопка, которая просит Rust открыть папку (`cmd`), и место для ошибки под ней. */
function openFolderButton(cmd: string, label: string, cls: string): HTMLElement {
  const msg = h("p", { class: "error", attrs: { role: "alert", "aria-live": "polite" } });
  msg.hidden = true;
  const btn = h("button", {
    class: cls,
    text: label,
    attrs: { type: "button" },
    on: {
      click: safe(async () => {
        msg.hidden = true;
        try {
          await call(cmd);
        } catch (e) {
          msg.textContent = errorText(asUiError(e));
          msg.hidden = false;
        }
      }),
    },
  });
  return h("div", { class: "openlib" }, btn, msg);
}

function sidebar(screen: Screen): HTMLElement {
  const idx = stepIndex(screen);
  const items = STEPS.map((label, i) => h("li", { class: idx < 0 ? "" : i < idx ? "done" : i === idx ? "now" : "", text: label }));
  // «Все копии» — всегда, и без входа
  const copies = !showCopiesLink(screen)
    ? null
    : h("button", {
    class: screen === "copies" ? "copies-link now" : "copies-link",
    text: "Все копии",
    attrs: { type: "button", "aria-current": screen === "copies" ? "page" : undefined },
    on: {
      click: () => {
        if (screen === "copies") return;
        state.beforeCopies = screen;
        go("copies");
      },
    },
  });
  const foot = h(
    "div",
    { class: "foot" },
    state.account ? h("div", { text: state.account.email }) : null,
    state.account ? openLibraryButton() : null,
    openLogButton(),
    state.update ? h("div", { class: "update", text: `Есть новая версия ${state.update}` }) : null,
    state.keychainNote ? h("div", { class: "update", text: state.keychainNote, attrs: { role: "status" } }) : null,
  );
  return h("nav", { class: "steps" }, h("div", { class: "brand" }, "Вернуть", h("br"), "приложения"), h("ol", {}, ...items), copies, foot);
}

/** Первый экран после запуска: список Apple ID, а если он пуст — сразу вход. */
export async function startScreen(): Promise<Screen> {
  try {
    const list = await call<AccountEntry[]>("list_accounts");
    return list.length ? "accounts" : "login";
  } catch {
    return "login";
  }
}

export function go(screen: Screen): void {
  current?.leave?.();
  state.screen = screen;
  const fn = screens.get(screen);
  if (!fn) throw new Error(`no screen ${screen}`);
  current = fn();
  const root = document.getElementById("app")!;
  root.replaceChildren(h("div", { class: "win" }, sidebar(screen), current.el));
  // Фокус на поле с autofocus, иначе на заголовок: экранный диктор начнёт с нового экрана.
  const target = current.el.querySelector<HTMLElement>("[autofocus]") ?? current.el.querySelector<HTMLElement>("h2");
  if (target?.tagName === "H2") target.setAttribute("tabindex", "-1");
  target?.focus();
}

/** Перерисовать только левую колонку (появился аккаунт или новая версия). */
export function refreshSidebar(): void {
  const old = document.querySelector(".steps");
  old?.replaceWith(sidebar(state.screen));
}

/** Ошибка из Rust или что-то неожиданное → UiError с понятным текстом. */
export function asUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "text" in e && typeof (e as UiError).text === "string") return e as UiError;
  console.error(e);
  return { code: "unknown", text: "Что-то пошло не так. Попробуйте ещё раз.", retryable: false };
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw asUiError(e);
  }
}
