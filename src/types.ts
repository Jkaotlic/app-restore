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
 * `store` — «В App Store»: найдено поиском по магазину (`search_store`).
 * `history` — «Из истории покупок»: нашлось на reportaproblem.apple.com, нет на «Выборе». */
export type RowSource = "removed" | "purchase" | "link" | "saved" | "phone" | "store" | "history";

/** Событие `history_progress`: сколько разных приложений iPhone уже нашлось. */
export interface HistoryProgress {
  found: number;
}

/** Событие `history`: окно закрыто; `rows` — новые строки «Из истории покупок». */
export interface HistoryOutcome {
  rows: RowDto[];
  found: number;
  error: string | null;
}

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
  /** Минимальная iOS того, что поставим (последняя версия, App Store); null — неизвестно. */
  min_ios: string | null;
  /** Только у `store`, ставит интерфейс: запрос, на который App Store вернул строку. */
  store_term?: string;
}

/** Версия в «Другая версия…» (`app_versions`). `display` null — Apple не успел ответить. */
export interface VersionDto {
  id: string;
  display: string | null;
  /** «ГГГГ-ММ-ДД». */
  released: string | null;
  /** Apple о ней спросили; false — не успели (пауза после 429). */
  checked: boolean;
}

/** Страница версий от новой к старой; `more` — есть старше; `error` — Apple перестал отвечать. */
export interface VersionsDto {
  versions: VersionDto[];
  more: boolean;
  error: UiError | null;
  /** Пауза после 429: через сколько секунд можно снова. */
  wait_s: number | null;
}

/** Итог `find_version`: самая новая версия, вышедшая до следующей iOS после телефона. */
export interface FoundVersionDto {
  version: VersionDto | null;
  next_ios: number | null;
  none_older: boolean;
  /** Запросы кончились, подходящей среди проверенных нет. */
  exhausted: boolean;
  error: UiError | null;
  wait_s: number | null;
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
  | { kind: "failed"; error: UiError; saved: boolean }
  /** Apple ответил 429/503: повтор скачивания через `seconds`. */
  | { kind: "waiting"; seconds: number }
  /** «Пауза» оборвала скачивание: после «Продолжить» начнётся заново. */
  | { kind: "paused" };

/** Незавершённое задание прошлого запуска (`pending_jobs`). */
export interface PendingJobDto {
  track_id: number;
  name: string;
  download_only: boolean;
  /** Примерный размер для «осталось ≈ N мин»; null — неизвестно. */
  size_bytes: number | null;
}

/** Итог «Продолжить» (`resume_pending`). */
export interface ResumeDto {
  jobs: PendingJobDto[];
  run: RunDto | null;
}

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

/** «Проверить весь каталог» (`check_catalog`, событие `catalog_check`): «проверено N из M».
 * `running: false` — проверка кончилась или остановлена. */
export interface SweepDto {
  checked: number;
  total: number;
  running: boolean;
}

/** Событие `dropped`: что перетащили в окно. `no_files` — не файл (текст или ссылка из браузера). */
export interface DropDto {
  copies: CopyDto[];
  ids: number[];
  errors: UiError[];
  no_files: boolean;
}
