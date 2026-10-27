// Формы данных от Rust (src-tauri/src/commands/logic.rs). Код там — правда.

export interface UiError {
  code: string;
  text: string;
  retryable: boolean;
  /** Только у `unknown`: код строки в журнале. */
  ref_code?: string | null;
}

export interface Account {
  email: string;
  name: string;
}

/** `phone` — «С этого iPhone»: стоит на телефоне, нет ни в каталоге, ни в покупках, ни в магазине RU.
 * `store` — «В App Store»: найдено поиском по магазину (`search_store`). */
export type RowSource = "removed" | "purchase" | "link" | "saved" | "phone" | "store";

/** Фильтр каталога; у покупок категории нет. */
export type Category = "bank" | "social" | "travel" | "other";

export interface RowDto {
  track_id: number;
  bundle_id: string;
  name: string;
  version: string;
  removed_from_ru: boolean;
  has_saved_copy: boolean;
  owned: boolean | null;
  source: RowSource;
  letter: string | null;
  color: string | null;
  /** «На iPhone: X»; null — не стоит или телефон не выбран. */
  installed_version: string | null;
  /** «Доступно: Y»; null — пока неизвестно. */
  available_version: string | null;
  install_state: InstallState;
  /** Отметить сразу (Rust: catalog::default_checked). */
  checked: boolean;
  /** «Только для iPad». */
  ipad_only: boolean;
  /** Копия без папки аккаунта и без покупателя: «Неизвестный Apple ID». */
  unknown_account: boolean;
  /** Другие названия записи каталога: по ним тоже ищет поиск. */
  aliases: string[];
  category: Category | null;
  /** Примерный размер в байтах; null — неизвестно. */
  size_bytes: number | null;
  /** «С этого iPhone», куплено на другом Apple ID: не выбрать. */
  other_account: boolean;
  /** Маска покупателя у `other_account` («ma***@example.com»); полного адреса нет. */
  purchaser_masked: string | null;
  /** Только у `store`, ставит интерфейс: запрос, на который App Store вернул строку. */
  store_term?: string;
}

export type InstallState = "not_installed" | "installed" | "update_available" | "unknown";

export interface RowsDto {
  rows: RowDto[];
  ru_check_failed: boolean;
  purchases_error: UiError | null;
  /** udid телефона-источника «С этого iPhone»; null — источника нет. */
  source_udid: string | null;
}

/** Итог `set_source_phone`: строки группы «С этого iPhone» и источник, по которому они построены. */
export interface SourceRowsDto {
  source_udid: string | null;
  rows: RowDto[];
}

export interface OwnedEvent {
  track_id: number;
  owned: boolean | null;
  error: UiError | null;
  available_version: string | null;
  install_state: InstallState;
  checked: boolean;
}

/** Событие `purchases_progress`. */
export interface PurchasesProgress {
  page: number;
  pages: number;
}

export interface InstalledDto {
  bundle_id: string;
  name: string;
  version: string;
  item_id: number | null;
}

export type JobDto =
  | { kind: "queued" }
  | { kind: "downloading"; bytes: number }
  | { kind: "saved" }
  | { kind: "installing"; percent: number }
  | { kind: "installed" }
  | { kind: "failed"; error: UiError; saved: boolean };

export interface JobEvent {
  track_id: number;
  state: JobDto;
  from_saved: string | null;
}

export interface PhoneDto {
  udid: string;
  name: string;
  ios_version: string;
  device_class: string;
  product_type: string;
  target: boolean;
}

export interface PhonesEvent {
  phones: PhoneDto[];
  error: UiError | null;
}

export interface RunDto {
  same_apple_id: boolean;
  email: string | null;
}

/** Чей Apple ID на телефоне против Apple ID программы. Адрес телефона только маской. */
/** Итог `switch_account`: адрес для поля входа и убранные задания. */
export interface SwitchDto {
  prefill: string | null;
  dropped: number[];
}

/** Итог `login`: аккаунт и trackId убранных заданий (вход в другой Apple ID). */
export interface LoginDto extends Account {
  dropped: number[];
}

export type AccountMatchDto = { kind: "same" } | { kind: "different"; phone_masked: string } | { kind: "unknown" };

/** Копия на экране «Все копии». Покупатель только маской; `path` — ключ для `install_copy`. */
export interface CopyDto {
  path: string;
  track_id: number | null;
  bundle_id: string;
  name: string;
  version: string;
  ipad_only: boolean;
  purchaser_masked: string | null;
}

/** Строка экрана «Apple ID». */
export interface AccountEntry {
  email: string;
  copies: number;
  /** «Вход сохранён»: есть снимок сессии или это нынешний вход. */
  saved: boolean;
  current: boolean;
}

/** Итог `use_account`: вошли снимком или нужен вход с подставленным адресом. */
export interface UseAccountDto {
  restored: boolean;
  account: Account | null;
  prefill: string | null;
  dropped: number[];
  /** Ничего не меняли (не сохранился нынешний вход): показать и остаться. */
  error: UiError | null;
  /** Ошибка на шаге сохранения нынешнего входа: можно «Перейти без сохранения». */
  can_skip_save: boolean;
}

/** Итог «Забыть». `error` — вышли (или не надо было), но снимок убрать не вышло. */
export interface ForgetDto {
  dropped: number[];
  signed_out: boolean;
  error: UiError | null;
}
