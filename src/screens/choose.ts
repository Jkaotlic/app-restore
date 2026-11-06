import { asUiError, call, go, refreshSidebar, register, safe, setSidebarExtra, state, targetIsIphone, targetPhone, type Job } from "../app";
import { h } from "../dom";
import { resumeBanner } from "./resume";
import { batchText, dropMessages, mergePending, runBatch } from "../drop";
import { HELP_STEPS, HISTORY_NOTE, LINK_NOTE, type HelpAction } from "../help";
import { purchasesProgressText, errorText } from "../labels";
import {
  addLinkRow,
  applyOwned,
  applyOwnedRow,
  barState,
  canSelect,
  dropStoreRows,
  replaceStoreRows,
  storeTerm,
  chipCounts,
  chipList,
  emptySearchText,
  foundText,
  groupPick,
  iconFor,
  idsToCheck,
  initialOwned,
  isLight,
  mergePurchases,
  pendingChecks,
  recheckIds,
  purchasesForSource,
  replacePhoneRows,
  sourceChoices,
  OTHER_ID_LOGIN_NOTE,
  SOURCE_HINT,
  HIDDEN_PURCHASES_HINT,
  rovingIndex,
  hiddenCount,
  selectionLabel,
  selectionSummary,
  ownedCell,
  phoneCell,
  subLine,
  rowA11y,
  sizeText,
  visibleGroups,
  visibleChecks,
  isNewRow,
  retryCatalogErrors,
  showSweepButton,
  sweepText,
  type Chip,
  type GroupKey,
  type CellTone,
  type OwnedMap,
} from "../rows";
import type { DropDto, FoundVersionDto, HistoryOutcome, HistoryProgress, InstalledDto, OwnedEvent, PurchasesProgress, RowDto, RowsDto, RunDto, SourceRowsDto, SweepDto, UiError, VersionDto, VersionsDto } from "../types";
import { canPickVersion, findOffered, foundMessage, recheckFrom, splitStart, startVersions, tooOldNote, versionLabel, waitText } from "../versions";
import { HISTORY_HINT, HISTORY_STEPS, historyProgressText, historyResultText, keptHistory, mergeHistoryRows, takeHistory } from "../history";

/** Значок: буква на цвете; `img` — иконка App Store поверх буквы (не загрузилась — снова буква). */
export function appIcon(r: RowDto, img?: HTMLImageElement | null): HTMLElement {
  const { letter, color } = iconFor(r);
  const cls = ["ico", [...letter].length > 2 ? "tight" : "", isLight(color) ? "light" : ""].filter(Boolean).join(" ");
  const ico = h("span", { class: cls, text: letter, style: { background: color }, attrs: { "aria-hidden": "true" } });
  if (img) ico.append(img);
  return ico;
}

/** `img` иконки; не загрузилась — убирается сам, `failed` — забыть адрес. */
function iconImgEl(url: string, failed: () => void): HTMLImageElement {
  const img = h("img", { attrs: { src: url, alt: "", loading: "lazy", referrerpolicy: "no-referrer" } });
  img.addEventListener("error", () => {
    img.remove();
    failed();
  });
  return img;
}

/** Телефон в карточке боковой панели. */
function sidePhoneIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ width: "30", height: "30", viewBox: "0 0 24 24", fill: "none", stroke: "#cfe0dd", "stroke-width": "1.5", "stroke-linecap": "round", "aria-hidden": "true" }))
    svg.setAttribute(k, v);
  const body = document.createElementNS(ns, "rect");
  for (const [k, v] of Object.entries({ x: "6.5", y: "2.5", width: "11", height: "19", rx: "2.5" })) body.setAttribute(k, v);
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", "M10.5 18.5h3");
  svg.append(body, p);
  return svg;
}

/** Заголовки разделов в боковой панели. */
const SECTION_TITLE: Record<Chip, string> = { all: "Разделы", bank: "Разделы", social: "Разделы", travel: "Разделы", buy: "Apple ID", on: "iPhone", new: "iPhone" };

function searchIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ width: "17", height: "17", viewBox: "0 0 24 24", fill: "none", stroke: "#5e7472", "stroke-width": "2.2", "stroke-linecap": "round", "aria-hidden": "true" }))
    svg.setAttribute(k, v);
  const c = document.createElementNS(ns, "circle");
  for (const [k, v] of Object.entries({ cx: "11", cy: "11", r: "7" })) c.setAttribute(k, v);
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", "m20 20-3.5-3.5");
  svg.append(c, p);
  return svg;
}

register("choose", () => {
  let rows: RowDto[] = [];
  let owned: OwnedMap = new Map();
  // «Назад» на шаг iPhone и обратно не сбрасывает галочки. Сама программа ничего не отмечает:
  // что вернуть, человек выбирает сам.
  const saved = state.choice;
  const selected = new Set<number>(saved?.selected);
  let started = false;
  let query = "";
  let chip: Chip = "all";
  let loaded = false;
  /** Покупки ещё грузятся; `progress` — последняя страница. */
  let purchasesLoading = true;
  let progress: PurchasesProgress | null = null;
  let dto: RowsDto | null = null;
  let installedError: UiError | null = null;
  /** Колонка «На iPhone»: телефон выбран и список с него прочитан. */
  let withPhone = false;
  /** Телефон-источник группы «С этого iPhone» (Rust: `source_udid`); меняет `set_source_phone`. */
  let sourceUdid: string | null = null;
  /** Источник, выбранный человеком; `null` — выбирает Rust. События покупок для другого источника перестраиваются. */
  let chosenSource: string | null = null;
  /** Номер последнего `set_source_phone`: ответ на прежний запрос не берём. */
  let sourceReq = 0;
  let sourceBusy = false;
  let sourceError: string | null = null;
  /** Поиск «В App Store»: идёт, не ответил или тихо. Номер запроса — ответ на прежний не берём. */
  let storeStatus: "idle" | "loading" | "error" = "idle";
  let storeReq = 0;
  /** Запрос, на который App Store уже ответил: пустая группа — «ничего не нашлось», а не сбой. */
  let storeAnswered: string | null = null;
  let storeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Видимые строки, что ждут проверки прав (`owned_visible`): дебаунс 300 мс, без повторов. */
  let visibleTimer: ReturnType<typeof setTimeout> | undefined;
  let visibleSent = "";
  /** «Проверить весь каталог»: последний прогресс; номер события — ответ команды старше события не берём. */
  let sweep: SweepDto | null = null;
  let sweepSeq = 0;
  let sweepError: string | null = null;
  /** Идёт добавление по ссылкам (поле или перетаскивание): следующие ждут в `state.pendingLinks`. */
  let linkBusy = false;
  /** «Отменить» пачку номеров; `left` — ушли с «Выбора»: пачка тоже останавливается. */
  let batchStop = false;
  let left = false;
  const cancelBtn = h("button", { class: "link", text: "Отменить", attrs: { type: "button" }, on: { click: () => (batchStop = true) } });
  cancelBtn.hidden = true;
  /** Подключённые телефоны (udid через запятую): выбор источника перерисовывается, только когда они меняются. */
  let phonesKey = state.phones.map((p) => p.udid).join(",");
  // «Только для iPad» зависит от устройства, на которое ставим, а не от «Дальше»
  const toIphone = () => targetIsIphone();

  /** Строки на экране по trackId и кнопки «Выбрать все в группе»: выбор меняется на месте. */
  const tiles = new Map<number, HTMLButtonElement>();
  /** «Другая версия…» под строками: видна у выбранных строк, которые скачиваются по номеру. */
  const versionLinks = new Map<number, HTMLButtonElement>();
  /** Открытые списки версий по trackId: переживают перерисовку списка. */
  /** «Найти подходящую версию» под несовместимыми строками: видна и без выбора. */
  const findLinks = new Map<number, HTMLButtonElement>();
  /** `waitUntil` — пауза после 429 (мс, `Date.now()`): кнопки к Apple скрыты, идёт обратный отсчёт. */
  type Panel = { loading: boolean; versions: VersionDto[]; more: boolean; error: string | null; found: string | null; waitUntil: number | null; listed: boolean };
  const waitTimers = new Map<number, ReturnType<typeof setInterval>>();
  const panels = new Map<number, Panel>();
  const panelEls = new Map<number, HTMLElement>();
  /** iOS телефона-цели, если ставим на iPhone: «Нужна iOS X, на iPhone Y». */
  const phoneIos = () => (toIphone() ? (targetPhone()?.ios_version ?? null) : null);
  const groupLinks: { key: GroupKey; rows: RowDto[]; btn: HTMLButtonElement }[] = [];
  /** Строки «Загружаем покупки: страница N из M» на экране: прогресс меняет только их текст. */
  const progressEls: HTMLElement[] = [];
  const progressEl = () => {
    const p = h("p", { class: "loading" }, h("span", { text: purchasesProgressText(progress) }), h("span", { class: "spin", attrs: { "aria-hidden": "true" } }));
    progressEls.push(p);
    return p;
  };

  // в живой области только короткий итог: весь список на каждую букву диктор не читает
  const list = h("div", { class: "scroll tiles-list" });
  const inspector = h("aside", { class: "inspector", attrs: { "aria-label": "О приложении", tabindex: "-1" } });
  // фокус на строке (Tab, стрелки, возврат фокуса) — она и в карточке
  list.addEventListener("focusin", (e) => {
    const t = (e.target as HTMLElement | null)?.closest?.<HTMLElement>(".tile");
    if (t?.dataset.id) setCurrent(Number(t.dataset.id));
  });
  // стрелки вверх/вниз, Home/End — между строками таблицы (видимыми сейчас)
  list.addEventListener("keydown", (e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    if (!t?.classList.contains("tile") || e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return;
    const all = [...list.querySelectorAll<HTMLElement>(".tile")];
    const i = all.length ? rovingIndex(e.key, all.indexOf(t), all.length) : null;
    if (i === null || e.key === "ArrowLeft" || e.key === "ArrowRight") return;
    e.preventDefault();
    all[i].focus();
    all[i].scrollIntoView({ block: "nearest" });
  });
  const found = h("span", { class: "found", attrs: { role: "status" } });
  // боковая панель: карточка телефона и разделы (бывшие чипсы)
  const phoneCard = h("div", { class: "side-phone" });
  const chips = h("div", { class: "sections", attrs: { role: "toolbar", "aria-label": "Разделы", "aria-orientation": "vertical" } });
  chips.addEventListener("keydown", (e: KeyboardEvent) => {
    const all = [...chips.querySelectorAll<HTMLButtonElement>(".sec-btn")];
    const i = rovingIndex(e.key, all.indexOf(document.activeElement as HTMLButtonElement), all.length);
    if (i === null || !all.length) return;
    e.preventDefault();
    for (const b of all) b.tabIndex = -1;
    all[i].tabIndex = 0;
    all[i].focus();
  });
  setSidebarExtra(h("div", { class: "side-choose" }, phoneCard, chips));

  /** Карточка телефона-цели: имя и iOS; «подключён» — только когда список с него прочитан (`withPhone`):
   * продолжили без iPhone или чтение не удалось — имя и iOS без «подключён». Без телефона — «iPhone не подключён». */
  function renderPhoneCard() {
    const p = targetPhone();
    phoneCard.classList.toggle("off", !p || !withPhone);
    phoneCard.replaceChildren(
      sidePhoneIcon(),
      h(
        "div",
        { class: "sp-t" },
        h("b", { text: p ? p.name : "iPhone не подключён" }),
        p ? h("span", { class: "sp-ios" }, withPhone ? h("i", { attrs: { "aria-hidden": "true" } }) : null, withPhone ? `iOS ${p.ios_version} · подключён` : `iOS ${p.ios_version}`) : null,
      ),
    );
  }
  renderPhoneCard();

  const search = h("input", {
    class: "input",
    attrs: {
      type: "search",
      placeholder: "Название, например «Сбер» или «ВК»",
      "aria-label": "Поиск по каталогу, покупкам и телефону",
      title: "Ищет везде, включая App Store, и по прежним названиям",
      autocomplete: "off",
      spellcheck: "false",
    },
    on: {
      input: () => {
        query = search.value;
        scheduleStore();
        renderChips();
        renderList();
      },
    },
  });

  const countEl = h("div", { class: "count", attrs: { "aria-live": "polite" } });
  const sizeEl = h("small", { class: "selsize" });
  const clearBtn = h("button", { class: "link", text: "Снять всё", attrs: { type: "button" }, on: { click: () => clearAll() } });
  const startBtn = h("button", { class: "btn go", attrs: { type: "button" }, on: { click: safe(() => start(false)) } });
  const onlyBtn = h("button", {
    class: "btn ghost",
    attrs: { type: "button", title: "Скачать копии в «Мои приложения», не ставя на телефон" },
    on: { click: safe(() => start(true)) },
  });
  const barHint = h("p", { class: "note bar-hint" });

  const linkInput = h("input", {
    class: "input",
    attrs: { type: "text", inputmode: "url", "aria-label": "Ссылка App Store", placeholder: "https://apps.apple.com/ru/app/…/id123456789", spellcheck: "false" },
  });
  const linkBtn = h("button", { class: "btn ghost small", text: "Добавить", attrs: { type: "submit" } });
  const linkMsg = h("p", { class: "error", attrs: { role: "alert", "aria-live": "polite" } });
  linkMsg.hidden = true;
  const linkForm = h(
    "form",
    { class: "fm-row", attrs: { novalidate: true, "aria-label": "Добавить по ссылке App Store" }, on: { submit: safe(addLink) } },
    h("div", { class: "fm-text" }, h("b", { text: "Есть ссылка App Store?" }), h("span", { class: "note", text: LINK_NOTE })),
    h("div", { class: "addlink" }, linkInput, linkBtn),
    linkMsg,
  );

  // «Проверить весь каталог»: кнопка и «проверено N из M» в заголовке каталога, один элемент на экран
  const sweepBtn = h("button", { class: "link", text: "Проверить весь каталог", attrs: { type: "button" }, on: { click: safe(checkCatalog) } });
  const sweepStatus = h("span", { class: "note", attrs: { role: "status" } });
  const sweepBox = h("div", { class: "sweepbox" }, sweepBtn, sweepStatus);

  const dropMsg = h("p", { class: "note drop-msg", attrs: { role: "status", "aria-live": "polite" } });
  dropMsg.hidden = true;
  // «Ещё способы» в блоке «Не нашли приложение?»: варианты, а не шаги — без номеров
  const tips = h(
    "ul",
    { class: "tips", attrs: { id: "find-tips" } },
    ...HELP_STEPS.map((st) =>
      h(
        "li",
        {},
        h("b", { text: st.title }),
        h("span", { text: ` ${st.text}` }),
        st.action ? h("button", { class: "link", text: st.action.label, attrs: { type: "button" }, on: { click: safe(() => helpGo(st.action!)) } }) : null,
      ),
    ),
  );
  tips.hidden = true;
  const tipsBtn = h("button", {
    class: "link tips-toggle",
    text: "Ещё способы найти",
    attrs: { type: "button", "aria-expanded": "false", "aria-controls": "find-tips" },
    on: {
      click: () => {
        tips.hidden = !tips.hidden;
        tipsBtn.setAttribute("aria-expanded", String(!tips.hidden));
        if (!tips.hidden) tipsBtn.scrollIntoView({ block: "start", behavior: "smooth" });
      },
    },
  });

  async function helpGo(a: HelpAction) {
    if (a.kind === "accounts") return go("accounts");
    if (a.kind === "phone") return go("phone");
    if (!purchasesLoading) await loadPurchases();
  }

  // «Найти всё, что вы скачивали»: окно Apple, вход — сам человек; сюда приходят только номера и названия.
  // Сначала три шага (что будет), потом окно; пока оно открыто — «Найдено N…», после — итог.
  type HistoryPhase = "idle" | "explain" | "open";
  let historyPhase: HistoryPhase = "idle";
  /** Итог захода истории — плашкой сверху списка (строка под блоком внизу, её не видно). */
  let historyNote: { text: string; error: boolean; show: boolean } | null = null;
  const historyBtn = h("button", {
    class: "btn ghost small",
    text: "Найти всё, что вы скачивали",
    attrs: { type: "button", "aria-controls": "history-steps" },
    on: { click: () => setHistoryPhase("explain") },
  });
  const historyOpenBtn = h("button", { class: "btn small", text: "Открыть страницу Apple", attrs: { type: "button" }, on: { click: safe(openHistory) } });
  const historyCancel = h("button", { class: "link", text: "Отмена", attrs: { type: "button" }, on: { click: () => setHistoryPhase("idle", true) } });
  const historyActions = h("div", { class: "hx-actions" }, historyOpenBtn, historyCancel);
  const historySteps = h(
    "div",
    { class: "hx", attrs: { id: "history-steps" } },
    h("ol", { class: "hx-steps" }, ...HISTORY_STEPS.map((t) => h("li", { text: t }))),
    h("p", { class: "note hx-safe", text: "Пароль вводится на странице Apple — программа его не видит и не хранит." }),
    historyActions,
  );
  historySteps.hidden = true;
  const historyMsg = h("p", { class: "note hx-msg", attrs: { role: "status", "aria-live": "polite" } });
  historyMsg.hidden = true;
  const historyRow = h(
    "div",
    { class: "fm-row" },
    h("div", { class: "fm-text" }, h("b", { text: "История покупок Apple" }), h("span", { class: "note", text: HISTORY_NOTE })),
    h("div", { class: "fm-act" }, historyBtn),
    historySteps,
    historyMsg,
  );
  const findHead = h("h3", { text: "Не нашли приложение?", attrs: { tabindex: "-1" } });
  const findMore = h("section", { class: "findmore", attrs: { "aria-label": "Не нашли приложение?" } }, findHead, historyRow, linkForm, h("div", { class: "fm-row fm-tips" }, tipsBtn, tips));
  const findLink = h("button", {
    class: "link find-link",
    text: "Не нашли приложение?",
    attrs: { type: "button", title: "К блоку под списком: история покупок Apple, ссылка App Store и другие способы" },
    on: {
      click: () => {
        findMore.scrollIntoView({ block: "start", behavior: "smooth" });
        findHead.focus({ preventScroll: true });
      },
    },
  });

  function setHistoryPhase(phase: HistoryPhase, focus = false) {
    historyPhase = phase;
    historySteps.hidden = phase === "idle";
    historyBtn.hidden = phase !== "idle";
    historyCancel.hidden = phase !== "explain";
    historyOpenBtn.textContent = phase === "open" ? "Показать окно Apple" : "Открыть страницу Apple";
    // пока окно открыто, «Найдено N…» — рядом с кнопкой окна, иначе — строкой под историей
    if (phase === "open") historyActions.append(historyMsg);
    else historyRow.append(historyMsg);
    if (phase === "explain") {
      historyMsg.hidden = true;
      historySteps.scrollIntoView({ block: "nearest" });
      historyOpenBtn.focus();
    }
    if (phase === "idle" && focus) historyBtn.focus();
  }

  /** Строка под историей: `busy` — со значком ожидания, `show` — кнопка «Показать» к группе. */
  function historyStatus(text: string, opts: { error?: boolean; busy?: boolean; show?: boolean } = {}) {
    historyMsg.className = opts.error ? "error hx-msg" : "note hx-msg";
    historyMsg.replaceChildren(h("span", { text }));
    if (opts.busy) historyMsg.append(h("span", { class: "spin", attrs: { "aria-hidden": "true" } }));
    if (opts.show)
      historyMsg.append(
        h("button", {
          class: "link",
          text: "Показать",
          attrs: { type: "button" },
          on: { click: showHistoryGroup },
        }),
      );
    historyMsg.hidden = false;
  }

  /** К группе «Из истории покупок»: прокрутка и фокус на первую строку. */
  function showHistoryGroup() {
    const sec = list.querySelector<HTMLElement>('section[data-group="history"]');
    sec?.scrollIntoView({ block: "start", behavior: "smooth" });
    sec?.querySelector<HTMLElement>(".tile")?.focus({ preventScroll: true });
  }

  async function openHistory() {
    const again = historyPhase === "open";
    historyNote = null;
    setHistoryPhase("open");
    if (!again) historyStatus(historyProgressText(0), { busy: true });
    try {
      // окно уже открыто — Rust только покажет его
      await call("open_history");
    } catch (e) {
      setHistoryPhase("idle");
      historyStatus(errorText(asUiError(e)), { error: true });
    }
  }

  function onHistoryProgress(p: HistoryProgress) {
    if (historyPhase !== "open") setHistoryPhase("open");
    historyStatus(historyProgressText(p.found), { busy: true });
  }

  function onHistory(o: HistoryOutcome) {
    setHistoryPhase("idle");
    historyBtn.textContent = "Найти ещё раз";
    const t = takeHistory(rows, owned, o.rows);
    rows = t.rows;
    owned = t.owned;
    const text = o.error ?? historyResultText(o.found, t.added.length);
    historyStatus(text, { error: o.error !== null, show: t.added.length > 0 });
    historyNote = { text, error: o.error !== null, show: t.added.length > 0 };
    if (loaded) {
      renderChips();
      renderList();
      // окно Apple закрылось само: итог должен быть виден сразу, сверху списка
      list.scrollTo({ top: 0 });
    }
    // новый вызов отменяет прежний: отправляем всё, что ещё ждёт проверки (только если есть новое)
    if (t.check) void call("check_owned", { trackIds: t.check }).catch(() => undefined);
  }

  const switchId = h("button", {
    class: "link",
    text: "Войти в другой Apple ID",
    attrs: { type: "button", title: "Список Apple ID: сохранённый вход подставится без пароля" },
    on: {
      // список Apple ID: сохранённый вход подставится без пароля; при смене аккаунта галочки сбросятся
      click: () => go("accounts"),
    },
  });

  const selectable = (r: RowDto) => canSelect(r, owned.get(r.track_id), toIphone());

  function setSelected(id: number, on: boolean) {
    if (on) selected.add(id);
    else selected.delete(id);
  }

  /** Ячейка «На этом Apple ID» и строка под названием. */
  const cellOf = (r: RowDto) => ownedCell(r, owned.get(r.track_id), toIphone());
  const subOf = (r: RowDto) => subLine(r, owned.get(r.track_id), toIphone(), { phoneIos: phoneIos(), chosen: state.versions.get(r.track_id) ?? null });

  /** Иконки App Store по trackId (`app_icons`), только найденные: промахи не запоминаем — Rust сам
   * помнит ответы Apple и на известные пустые отвечает без сети. `iconsAsked` — номера в идущем вызове. */
  const icons = new Map<number, string>();
  const iconsAsked = new Set<number>();
  /** Узлы `img` по месту («row»/«big») и trackId: перерисовка переносит тот же узел — без мигания. */
  const imgs = new Map<string, HTMLImageElement>();
  function iconImg(id: number, slot: "row" | "big"): HTMLImageElement | null {
    const url = icons.get(id);
    if (!url) return null;
    const key = `${slot}:${id}`;
    let img = imgs.get(key);
    if (!img) {
      img = iconImgEl(url, () => {
        imgs.delete(key);
        // адрес не грузится — дальше буква, второй раз не спрашиваем
        icons.set(id, "");
      });
      imgs.set(key, img);
    }
    return img;
  }
  /** «На iPhone» или «На iPad» — куда ставим (заголовок колонки и карточка). */
  const deviceCol = () => (targetPhone()?.device_class === "iPad" ? "На iPad" : "На iPhone");
  /** Карточка справа: последняя строка, по которой кликнули или на которую перешёл фокус. */
  let curId: number | null = null;

  /** Строка таблицы: флажок · иконка, название и подстрока · Apple ID · iPhone · размер. */
  function tileEl(r: RowDto): HTMLButtonElement {
    const own = cellOf(r);
    const sub = subOf(r);
    const ph = phoneCell(r, withPhone);
    const enabled = selectable(r);
    const tile = h(
      "button",
      {
        class: r.track_id === curId ? "tile cur" : "tile",
        // tabindex явно: WebKit без него пропускает кнопки по Tab (macOS «Навигация с клавиатуры» выключена по умолчанию)
        attrs: { type: "button", tabindex: "0", title: [r.name, own.text, sub.text, sub.note].filter(Boolean).join(" · ") },
        on: {
          // `aria-disabled`, не `disabled`: строка в порядке Tab и называет причину; нажатие ничего не делает
          click: safe(async () => {
            setCurrent(r.track_id);
            if (own.tone === "retry") return recheck(r.track_id);
            if (!selectable(r)) return;
            setSelected(r.track_id, !selected.has(r.track_id));
            syncSelection();
          }),
        },
      },
      h("span", { class: "tbox", attrs: { "aria-hidden": "true" } }),
      h(
        "span",
        { class: "c-app" },
        appIcon(r, iconImg(r.track_id, "row")),
        h(
          "span",
          { class: "tt" },
          h("span", { class: "nm" }, isNewRow(r, state.newIds) ? h("span", { class: "badge-new", text: "Новое" }) : null, r.name),
          h("span", { class: `sub ${sub.tone}`, text: sub.text }),
          sub.note ? h("span", { class: "rnote", text: sub.note }) : null,
        ),
      ),
      ownEl(own),
      h("span", { class: "c-phone" }, h("span", { text: ph.have }), ph.newer ? h("span", { class: "up", text: `→ ${ph.newer}` }) : null),
      h("span", { class: "c-size", text: sizeText(r.size_bytes) }),
    );
    tile.dataset.id = String(r.track_id);
    applyA11y(tile, r, own, enabled);
    return tile;
  }

  /** Плашка «На этом Apple ID» по тону; пустой текст — пустая ячейка. */
  function ownEl(own: { text: string; tone: CellTone }): HTMLElement {
    return h(
      "span",
      { class: "c-own" },
      own.tone === "checking" ? h("span", { class: "spin", attrs: { "aria-hidden": "true" } }) : null,
      own.text ? h("span", { class: `pill ${own.tone}`, text: own.text }) : null,
    );
  }

  /** Строка группы «С этого iPhone» и её кнопки под ней: внутри кнопки-строки другие кнопки нельзя. */
  function phoneTile(r: RowDto, tile: HTMLButtonElement): HTMLElement {
    const msg = h("span", { class: "note", attrs: { role: "status" } });
    const report = h("button", {
      class: "link",
      text: "Сообщить в каталог",
      attrs: { type: "button", title: "Откроет на GitHub заявку с номером, названием и версией приложения" },
      on: {
        click: safe(async () => {
          msg.textContent = "";
          try {
            await call("report_to_catalog", { trackId: r.track_id, name: r.name, version: r.version, bundleId: r.bundle_id });
            msg.textContent = "Открыто в браузере";
          } catch (e) {
            msg.textContent = errorText(asUiError(e));
          }
        }),
      },
    });
    const login = r.other_account
      ? h("button", {
          class: "link",
          text: "Войти в этот Apple ID",
          attrs: { type: "button" },
          on: {
            click: () => {
              state.loginPrefill = null;
              state.addingAccount = true;
              state.loginNote = OTHER_ID_LOGIN_NOTE;
              go("login");
            },
          },
        })
      : null;
    return h("div", { class: "tilewrap" }, tile, h("div", { class: "tile-actions" }, login, report, msg));
  }

  /** Строка нового выпуска («Новое»): под ней «Открыть в App Store» — лицензию человек берёт сам. */
  function newTile(r: RowDto, tile: HTMLButtonElement): HTMLElement {
    const msg = h("span", { class: "note", attrs: { role: "status" } });
    const open = h("button", {
      class: "link",
      text: "Открыть в App Store",
      attrs: { type: "button", title: "Откроет страницу приложения в App Store: возьмите его там, потом программа сохранит копию" },
      on: {
        click: safe(async () => {
          msg.textContent = "";
          try {
            await call("open_in_store", { trackId: r.track_id });
            msg.textContent = "Открыто в браузере";
          } catch (e) {
            msg.textContent = errorText(asUiError(e));
          }
        }),
      },
    });
    return h("div", { class: "tilewrap" }, tile, h("div", { class: "tile-actions" }, open, msg));
  }

  /** «Телефон-источник» над группой: другой подключённый телефон, цель установки та же. */
  function sourcePicker(choices: { udid: string; label: string; selected: boolean }[]): HTMLElement {
    const select = h("select", { class: "input small", attrs: { "aria-label": "Телефон-источник" } });
    if (!choices.some((c) => c.selected)) select.append(h("option", { text: "—", attrs: { value: "", selected: true, disabled: true } }));
    for (const c of choices) select.append(h("option", { text: c.label, attrs: { value: c.udid, selected: c.selected } }));
    select.disabled = sourceBusy;
    select.addEventListener("change", safe(() => changeSource(select.value)));
    const parts: HTMLElement[] = [h("label", { class: "source" }, h("span", { text: "Телефон-источник" }), select)];
    if (sourceBusy) parts.push(h("span", { class: "spin", attrs: { "aria-hidden": "true" } }));
    const box = h("div", { class: "sourcebox" }, ...parts);
    if (sourceError) box.append(h("p", { class: "error", text: sourceError, attrs: { role: "alert" } }));
    return box;
  }

  async function changeSource(udid: string) {
    if (!udid || udid === sourceUdid) return;
    await loadSource(udid);
    list.querySelector<HTMLSelectElement>(".sourcebox select")?.focus();
  }

  /** Строки группы по источнику `udid`. Берётся только ответ на последний запрос и только для этого источника. */
  async function loadSource(udid: string) {
    const req = ++sourceReq;
    chosenSource = udid;
    sourceBusy = true;
    sourceError = null;
    renderList();
    try {
      const res = await call<SourceRowsDto>("set_source_phone", { udid });
      if (req !== sourceReq) return;
      if (res.source_udid !== udid) throw new Error("source mismatch");
      const incoming = res.rows;
      sourceUdid = udid;
      rows = replacePhoneRows(rows, incoming);
      for (const r of incoming) if (!owned.has(r.track_id)) owned.set(r.track_id, initialOwned([r]).get(r.track_id)!);
      // новый вызов отменяет прежний: отправляем всё, что ещё ждёт проверки
      if (idsToCheck(incoming).length) void call("check_owned", { trackIds: pendingChecks(rows, owned) }).catch(() => undefined);
    } catch (e) {
      if (req !== sourceReq) return;
      sourceError = errorText(asUiError(e));
    }
    sourceBusy = false;
    renderChips();
    renderList();
  }

  /** Пауза 400 мс после ввода; новый ввод отменяет прежний запрос (его ответ не берём). */
  function scheduleStore() {
    clearTimeout(storeTimer);
    const term = storeTerm(query);
    storeReq++;
    if (term === null) {
      storeStatus = "idle";
      rows = dropStoreRows(rows, selected);
      return;
    }
    storeStatus = "loading";
    storeTimer = setTimeout(() => void searchStore(term), 400);
  }

  async function searchStore(term: string) {
    const req = ++storeReq;
    try {
      const found = await call<RowDto[]>("search_store", { term });
      if (req !== storeReq) return;
      rows = replaceStoreRows(rows, found, selected, term);
      storeAnswered = term;
      for (const r of rows) if (!owned.has(r.track_id)) owned.set(r.track_id, initialOwned([r]).get(r.track_id)!);
      storeStatus = "idle";
      // новый вызов отменяет прежний: отправляем всё, что ещё ждёт проверки; `recheck` — Rust
      // допишет устаревшие ответы кэша (и найденных в App Store), даже если ждущих нет
      if (found.length) void call("check_owned", { trackIds: pendingChecks(rows, owned), recheck: true }).catch(() => undefined);
    } catch {
      if (req !== storeReq) return;
      storeStatus = "error";
    }
    if (!loaded) return;
    renderChips();
    renderList();
  }

  function applyA11y(tile: HTMLElement, r: RowDto, own: { text: string; tone: CellTone }, enabled: boolean) {
    const a = rowA11y(r.name, own, enabled, selected.has(r.track_id));
    if (a.pressed === null) tile.removeAttribute("aria-pressed");
    else tile.setAttribute("aria-pressed", a.pressed);
    if (a.disabled) tile.setAttribute("aria-disabled", "true");
    else tile.removeAttribute("aria-disabled");
    if (a.label) tile.setAttribute("aria-label", a.label);
    else tile.removeAttribute("aria-label");
  }

  /** Отметки, «Выбрать все в группе» и нижняя панель — без перестройки списка (фокус на месте). */
  function syncSelection() {
    const byId = new Map(rows.map((r) => [r.track_id, r]));
    for (const [id, tile] of tiles) {
      const r = byId.get(id);
      if (r) applyA11y(tile, r, cellOf(r), selectable(r));
    }
    for (const [id, link] of versionLinks) {
      const r = byId.get(id);
      link.hidden = !r || !versionOffered(r);
    }
    for (const [id, link] of findLinks) {
      const r = byId.get(id);
      link.hidden = !r || !findOffered(r, owned.get(id), phoneIos(), state.versions.get(id));
    }
    for (const g of groupLinks) {
      const pick = groupPick(g.rows, selectable, selected);
      g.btn.hidden = pick.ids.length === 0;
      g.btn.textContent = pick.allOn ? "Снять в группе" : "Выбрать все в группе";
    }
    renderBar();
    renderInspector();
  }

  /** Строка для карточки справа; подсветка — у неё одной. */
  function setCurrent(id: number) {
    if (curId === id) return;
    curId = id;
    for (const [tid, t] of tiles) t.classList.toggle("cur", tid === id);
    renderInspector();
  }

  /** У строки бывает «Другая версия…»: скачивается по номеру со своего Apple ID. */
  const versionRow = (r: RowDto) => r.track_id > 0 && !r.other_account && r.source !== "saved";
  /** «Другая версия…» видна (под строкой и в карточке): такая строка выбрана и версию можно выбрать. */
  const versionOffered = (r: RowDto) => versionRow(r) && selected.has(r.track_id) && canPickVersion(r, owned.get(r.track_id));

  /** Карточка справа (видна от 1180 px): иконка, название, подстрока, факты, «Другая версия…». */
  function renderInspector() {
    // строка, скрытая фильтром, в карточке не показывается: панель версий открылась бы невидимой
    const r = curId !== null && tiles.has(curId) ? rows.find((x) => x.track_id === curId) : undefined;
    const hadFocus = inspector.contains(document.activeElement);
    // кнопки в карточке больше нет — фокус на строку, иначе на саму карточку (не на body)
    const keepFocus = (btn: HTMLElement | null) => {
      if (!hadFocus) return;
      if (btn) btn.focus();
      else if (curId !== null && tiles.has(curId)) tiles.get(curId)!.focus();
      else inspector.focus();
    };
    if (!r) {
      inspector.replaceChildren(h("p", { class: "insp-empty", text: "Нажмите на приложение, чтобы увидеть подробности" }));
      keepFocus(null);
      return;
    }
    const own = cellOf(r);
    const sub = subOf(r);
    const ph = phoneCell(r, withPhone);
    const big = appIcon(r, iconImg(r.track_id, "big"));
    big.classList.add("big");
    const fact = (k: string, v: string) => [h("dt", { text: k }), h("dd", { text: v })];
    const parts: HTMLElement[] = [
      h("div", { class: "insp-head" }, big, h("div", { class: "insp-t" }, h("h3", { text: r.name }), h("p", { class: `sub ${sub.tone}`, text: sub.text }))),
      h(
        "dl",
        { class: "insp-facts" },
        ...fact("Apple ID", own.text || "—"),
        ...fact("Доступно", r.available_version || "—"),
        ...fact(deviceCol(), ph.newer ? `${ph.have} → ${ph.newer}` : ph.have),
        ...fact("Размер", sizeText(r.size_bytes)),
        ...fact("Нужна iOS", r.min_ios || "—"),
      ),
    ];
    if (sub.note) parts.push(h("p", { class: "note", text: sub.note }));
    const ver = versionOffered(r) ? h("button", { class: "btn ghost small", text: "Другая версия…", attrs: { type: "button" }, on: { click: safe(() => openVersions(r)) } }) : null;
    if (ver) parts.push(ver);
    inspector.replaceChildren(...parts);
    // карточка перерисовывается на каждое событие: фокус с её кнопки не теряем
    keepFocus(ver);
  }

  /** Иконки для строк на экране без иконки, которых сейчас не спрашивают: по 1000 номеров за вызов.
   * Сбой — молча, буквы; следующая отрисовка спросит снова. */
  async function fetchIcons() {
    const all = [...tiles.keys()].filter((id) => id > 0 && !icons.has(id) && !iconsAsked.has(id));
    // все сразу: следующая отрисовка, пока идут пачки, не пошлёт их второй раз
    for (const id of all) iconsAsked.add(id);
    for (let i = 0; i < all.length; i += 1000) {
      const ids = all.slice(i, i + 1000);
      let got: Record<string, string> = {};
      try {
        got = await call<Record<string, string>>("app_icons", { trackIds: ids });
      } catch {
        // молча: остаются буквы
      }
      for (const id of ids) {
        iconsAsked.delete(id);
        const url = got[String(id)];
        if (url) icons.set(id, url);
      }
      if (left) {
        for (const id of all) iconsAsked.delete(id);
        return;
      }
      for (const id of ids) {
        const img = iconImg(id, "row");
        const ico = img ? tiles.get(id)?.querySelector(".ico") : null;
        if (img && ico && !ico.contains(img)) ico.append(img);
      }
      if (curId !== null && ids.includes(curId)) renderInspector();
    }
  }

  function renderChips() {
    const counts = chipCounts(rows, query, withPhone);
    const keys = chipList(withPhone);
    if (!keys.some((c) => c.key === chip)) chip = "all";
    const focused = (document.activeElement as HTMLElement | null)?.dataset?.chip;
    const parts: HTMLElement[] = [];
    let group = h("div");
    let title = "";
    for (const c of keys) {
      // разделы одной темы — группа с названием: «Разделы», «Apple ID», «iPhone»
      if (SECTION_TITLE[c.key] !== title) {
        title = SECTION_TITLE[c.key];
        group = h("div", { class: "sec-group", attrs: { role: "group", "aria-label": title } }, h("div", { class: "sec-title", text: title, attrs: { "aria-hidden": "true" } }));
        parts.push(group);
      }
      const b = h(
        "button",
        {
          class: "sec-btn",
          attrs: { type: "button", "aria-pressed": String(c.key === chip) },
          on: {
            click: () => {
              chip = c.key;
              renderChips();
              renderList();
            },
          },
        },
        h("span", { class: "sec-l", text: c.label }),
        h("span", { class: "n", text: String(counts[c.key]) }),
      );
      b.dataset.chip = c.key;
      // в Tab один раздел (выбранный), по остальным — стрелками
      b.tabIndex = c.key === chip ? 0 : -1;
      group.append(b);
    }
    chips.replaceChildren(...parts);
    if (focused) chips.querySelector<HTMLElement>(`[data-chip="${focused}"]`)?.focus();
  }

  /** Группа: заголовок (`head` — кнопки рядом с «Выбрать все в группе»), `extra` — под заголовком. */
  function groupSection(key: GroupKey, title: string, items: RowDto[], extra: HTMLElement[], head: HTMLElement[] = []): HTMLElement {
    const btn = h("button", { class: "link", attrs: { type: "button" } });
    btn.addEventListener("click", () => {
      const pick = groupPick(items, selectable, selected);
      for (const id of pick.ids) setSelected(id, !pick.allOn);
      syncSelection();
    });
    groupLinks.push({ key, rows: items, btn });
    const grid = h("div", { class: "trows" });
    for (const r of items) {
      const t = tileEl(r);
      tiles.set(r.track_id, t);
      const cell = r.source === "phone" ? phoneTile(r, t) : isNewRow(r, state.newIds) ? newTile(r, t) : t;
      grid.append(withVersionLink(r, cell));
      if (panels.has(r.track_id)) grid.append(panelEl(r));
    }
    const sec = h(
      "section",
      { class: "gsec", attrs: { "aria-label": title } },
      h("div", { class: "ghead" }, h("h3", {}, title, h("small", { text: String(items.length) })), h("div", { class: "gacts" }, ...head, btn)),
      ...extra,
      items.length ? grid : null,
    );
    sec.dataset.group = key;
    return sec;
  }

  function banners(): HTMLElement[] {
    const parts: HTMLElement[] = [];
    if (installedError) parts.push(h("p", { class: "note", text: "Не удалось прочитать, что стоит на iPhone" }));
    const pe = dto?.purchases_error;
    if (pe) {
      const b = h("div", { class: "banner" }, h("span", { text: pe.text }));
      if (pe.code === "not_logged_in" || pe.code === "apple_changed_login") {
        b.append(h("button", { class: "btn ghost small", text: "Войти снова", attrs: { type: "button" }, on: { click: () => go("login") } }));
      } else {
        b.append(h("button", { class: "btn ghost small", text: "Загрузить снова", attrs: { type: "button" }, on: { click: safe(loadPurchases) } }));
      }
      parts.push(b);
    }
    if (dto?.ru_check_failed) parts.push(h("p", { class: "note", text: "Не удалось проверить российский App Store" }));
    if (historyNote) {
      const note = historyNote;
      const b = h("div", { class: note.error ? "banner bad" : "banner", attrs: { role: "status" } }, h("span", { text: note.text }));
      if (note.show) b.append(h("button", { class: "btn go small", text: "Показать", attrs: { type: "button" }, on: { click: showHistoryGroup } }));
      b.append(
        h("button", {
          class: "link",
          text: "Скрыть",
          attrs: { type: "button" },
          on: {
            click: () => {
              historyNote = null;
              renderList();
            },
          },
        }),
      );
      parts.unshift(b);
    }
    return parts;
  }

  /** «Другая версия…» под строкой (у строки-кнопки — своя обёртка: кнопку в кнопку нельзя). */
  function withVersionLink(r: RowDto, cell: HTMLElement): HTMLElement {
    if (!versionRow(r)) return cell;
    const link = h("button", {
      class: "link",
      text: "Другая версия…",
      attrs: { type: "button", title: `Выбрать, какую версию «${r.name}» скачать` },
      on: { click: safe(() => openVersions(r)) },
    });
    link.hidden = true;
    versionLinks.set(r.track_id, link);
    // последняя не запустится: сразу предложить ту, что запустится
    const find = h("button", {
      class: "link",
      text: "Найти подходящую версию",
      attrs: { type: "button", title: `Найти самую новую версию «${r.name}», которая запустится на этом iPhone` },
      on: { click: safe(() => openVersions(r, true)) },
    });
    find.hidden = true;
    findLinks.set(r.track_id, find);
    if (cell.classList.contains("tilewrap")) {
      cell.querySelector(".tile-actions")?.prepend(find, link);
      return cell;
    }
    return h("div", { class: "tilewrap" }, cell, h("div", { class: "tile-actions" }, find, link));
  }

  /** Сколько секунд ещё пауза после 429; 0 — можно спрашивать. */
  const waitLeft = (p: Panel) => (p.waitUntil === null ? 0 : Math.max(0, Math.ceil((p.waitUntil - Date.now()) / 1000)));

  /** Пауза после 429 с обратным отсчётом: раз в секунду перерисовать, по окончании — кнопки назад. */
  function startWait(id: number, seconds: number | null) {
    const p = panels.get(id);
    if (!p || !seconds) return;
    p.waitUntil = Date.now() + seconds * 1000;
    clearInterval(waitTimers.get(id));
    waitTimers.set(
      id,
      setInterval(() => {
        const cur = panels.get(id);
        if (!cur || waitLeft(cur) === 0) {
          clearInterval(waitTimers.get(id));
          waitTimers.delete(id);
          if (cur) cur.waitUntil = null;
        }
        refreshPanel(id);
      }, 1000),
    );
  }

  /** Список версий под строкой: «Последняя», версии от новой к старой, «Ещё» и «Найти подходящую». */
  function panelEl(r: RowDto): HTMLElement {
    const id = r.track_id;
    const p = panels.get(id)!;
    const chosenId = state.versions.get(id)?.id ?? null;
    const pick = (v: VersionDto | null) => {
      if (v) state.versions.set(id, { id: v.id, display: v.display });
      else state.versions.delete(id);
      if (!selected.has(id) && selectable(r)) setSelected(id, true);
      renderTile(id);
      refreshPanel(id);
      syncSelection();
    };
    const option = (text: string, v: VersionDto | null) =>
      h("button", { class: "veropt", text, attrs: { type: "button", "aria-pressed": String((v?.id ?? null) === chosenId) }, on: { click: () => pick(v) } });
    const parts: HTMLElement[] = [h("p", { class: "note", text: `Какую версию «${r.name}» скачать` })];
    const list = h("div", { class: "veropts", attrs: { role: "group", "aria-label": `Версии ${r.name}` } }, option("Последняя (по умолчанию)", null), ...p.versions.map((v) => option(versionLabel(v), v)));
    parts.push(list);
    if (p.loading) parts.push(h("p", { class: "loading" }, "Спрашиваем Apple…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
    const wait = waitText(waitLeft(p));
    if (wait) parts.push(h("p", { class: "error", text: wait, attrs: { role: "status" } }));
    else if (p.error) parts.push(h("p", { class: "error", text: p.error, attrs: { role: "alert" } }));
    if (p.found) parts.push(h("p", { class: "note", text: p.found, attrs: { role: "status" } }));
    const actions = h("div", { class: "tile-actions" });
    const free = !p.loading && !wait;
    const again = recheckFrom(p.versions);
    if (again && free) actions.append(h("button", { class: "link", text: "Проверить остальные", attrs: { type: "button" }, on: { click: safe(() => loadVersions(r, again.before)) } }));
    if (!p.listed && free) actions.append(h("button", { class: "link", text: "Показать все версии", attrs: { type: "button" }, on: { click: safe(() => loadVersions(r, null)) } }));
    if (p.more && free && !again) actions.append(h("button", { class: "link", text: "Ещё старше", attrs: { type: "button" }, on: { click: safe(() => loadVersions(r, p.versions[p.versions.length - 1]?.id ?? null)) } }));
    const ios = phoneIos();
    if (ios && free) actions.append(h("button", { class: "link", text: `Найти подходящую для iOS ${ios}`, attrs: { type: "button" }, on: { click: safe(() => findVersion(r, ios)) } }));
    actions.append(
      h("button", {
        class: "link",
        text: "Закрыть",
        attrs: { type: "button" },
        on: {
          click: () => {
            panels.delete(id);
            clearInterval(waitTimers.get(id));
            waitTimers.delete(id);
            panelEls.get(id)?.remove();
            panelEls.delete(id);
            versionLinks.get(id)?.focus();
          },
        },
      }),
    );
    parts.push(actions);
    const el = h("div", { class: "verpanel" }, ...parts);
    panelEls.set(id, el);
    return el;
  }

  function refreshPanel(id: number) {
    const old = panelEls.get(id);
    const r = rows.find((x) => x.track_id === id);
    if (!old || !r || !panels.has(id)) return;
    const focused = old.contains(document.activeElement) ? (document.activeElement as HTMLElement).textContent : null;
    const el = panelEl(r);
    old.replaceWith(el);
    if (focused) [...el.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === focused)?.focus();
  }

  /** Открыть список версий; `find` — сразу искать подходящую под iOS телефона. */
  async function openVersions(r: RowDto, find = false) {
    const ios = phoneIos();
    if (panels.has(r.track_id)) {
      // уже открыт (например, «Другая версия…» в карточке): показать его и перейти в него
      const el = panelEls.get(r.track_id);
      el?.scrollIntoView({ block: "nearest", behavior: "smooth" });
      (el?.querySelector<HTMLButtonElement>('.veropt[aria-pressed="true"]') ?? el?.querySelector<HTMLButtonElement>(".veropt"))?.focus({ preventScroll: true });
      if (find && ios) await findVersion(r, ios);
      return;
    }
    panels.set(r.track_id, { loading: true, versions: [], more: false, error: null, found: null, waitUntil: null, listed: false });
    renderList();
    panelEls.get(r.track_id)?.querySelector<HTMLButtonElement>(".veropt")?.focus();
    if (find && ios) await findVersion(r, ios);
    else await loadVersions(r, null);
  }

  /** Страница версий у Apple (не больше 10 запросов за раз; 429 — остановка и объяснение). */
  async function loadVersions(r: RowDto, before: string | null) {
    const p = panels.get(r.track_id);
    if (!p) return;
    p.loading = true;
    p.error = null;
    refreshPanel(r.track_id);
    try {
      const d = await call<VersionsDto>("app_versions", { trackId: r.track_id, before });
      p.listed = true;
      // переспрошенные заменяют свои «не проверено», новые — в конец
      const fresh = new Map(d.versions.map((v) => [v.id, v]));
      p.versions = [...p.versions.map((v) => fresh.get(v.id) ?? v), ...d.versions.filter((v) => !p.versions.some((x) => x.id === v.id))];
      if (before === null || p.versions[p.versions.length - 1]?.id === d.versions[d.versions.length - 1]?.id) p.more = d.more;
      p.error = d.error ? errorText(d.error) : null;
      startWait(r.track_id, d.wait_s);
    } catch (e) {
      p.error = errorText(asUiError(e));
    }
    p.loading = false;
    refreshPanel(r.track_id);
  }

  /** «Найти подходящую»: самая новая версия, вышедшая до следующей iOS; найденная сразу выбрана. */
  async function findVersion(r: RowDto, ios: string) {
    const p = panels.get(r.track_id);
    if (!p) return;
    p.loading = true;
    p.found = null;
    p.error = null;
    refreshPanel(r.track_id);
    try {
      const f = await call<FoundVersionDto>("find_version", { trackId: r.track_id });
      p.found = f.wait_s ? null : foundMessage(f, ios);
      if (f.wait_s) p.error = f.error ? errorText(f.error) : null;
      startWait(r.track_id, f.wait_s);
      if (f.version) {
        const v = f.version;
        if (!p.versions.some((x) => x.id === v.id)) p.versions = [...p.versions, v];
        state.versions.set(r.track_id, { id: v.id, display: v.display });
        if (!selected.has(r.track_id) && selectable(r)) setSelected(r.track_id, true);
        renderTile(r.track_id);
      }
    } catch (e) {
      p.error = errorText(asUiError(e));
    }
    p.loading = false;
    refreshPanel(r.track_id);
    syncSelection();
  }

  function renderList() {
    tiles.clear();
    versionLinks.clear();
    findLinks.clear();
    panelEls.clear();
    groupLinks.length = 0;
    progressEls.length = 0;
    if (!loaded) {
      list.replaceChildren(h("p", { class: "loading" }, "Открываем…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
      renderBar();
      return;
    }
    const focusedId = (document.activeElement as HTMLElement | null)?.closest?.<HTMLElement>(".tile")?.dataset.id;
    const groups = visibleGroups(rows, chip, query, withPhone);
    reportVisible(visibleChecks(groups, owned));
    found.textContent = foundText(rows, chip, query, withPhone);
    const plain = chip === "all" && query.trim() === "";
    const parts: HTMLElement[] = banners();
    // колонки — один закреплённый заголовок над всеми группами
    const table = h(
      "div",
      { class: "tbl" },
      h("div", { class: "thead", attrs: { "aria-hidden": "true" } }, h("span"), h("span", { text: "Приложение" }), h("span", { text: "На этом Apple ID" }), h("span", { text: deviceCol() }), h("span", { text: "Размер" })),
    );
    parts.push(table);
    for (const g of groups) {
      const extra: HTMLElement[] = [];
      if (g.key === "catalog" && !g.rows.length && plain) extra.push(h("p", { class: "empty", text: "Пока пусто. Добавьте приложение по ссылке — блок «Не нашли приложение?» ниже." }));
      const head: HTMLElement[] = [];
      if (g.key === "catalog" && g.rows.length && state.account) {
        renderSweep();
        head.push(sweepBox);
      }
      if (g.key === "buy" && plain) {
        const purchases = rows.filter((r) => r.source === "purchase" || r.source === "saved");
        if (purchasesLoading) {
          extra.push(progressEl());
        } else if (purchases.length === 0 && !dto?.purchases_error) {
          extra.push(h("p", { class: "empty", text: "На этом Apple ID нет покупок для iPhone. Проверьте, тот ли это Apple ID." }));
        }
      }
      // подсказка группы — под заголовком, до плиток: сначала что это, потом список
      if (g.key === "store") {
        if (storeStatus === "loading") extra.push(h("p", { class: "loading" }, "Ищем в App Store…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
        else if (storeStatus === "error") extra.push(h("p", { class: "note", text: "App Store не ответил" }));
        else if (!g.rows.length && storeAnswered !== null && storeAnswered === storeTerm(query)) extra.push(h("p", { class: "empty", text: "В App Store ничего не нашлось" }));
        if (g.rows.length) extra.push(h("p", { class: "note group-hint", text: HIDDEN_PURCHASES_HINT }));
      }
      if (g.key === "phone" && !purchasesLoading) {
        const choices = sourceChoices(state.phones, sourceUdid);
        if (choices && (plain || g.rows.length)) extra.push(sourcePicker(choices));
        if (choices && plain && !g.rows.length) extra.push(h("p", { class: "empty", text: "На этом телефоне нет приложений, которых нет в каталоге и в российском App Store." }));
        if (g.rows.length) extra.push(h("p", { class: "note group-hint", text: SOURCE_HINT }));
      }
      if (g.key === "history" && g.rows.length) extra.push(h("p", { class: "note group-hint", text: HISTORY_HINT }));
      if (g.rows.length || extra.length) table.append(groupSection(g.key, g.title, g.rows, extra, head));
    }
    if (groups.every((g) => !g.rows.length) && !plain) {
      parts.push(h("p", { class: "empty", text: query.trim() ? emptySearchText(query) : "В этом фильтре пусто." }));
      if (purchasesLoading) parts.push(progressEl());
    }
    parts.push(findMore);
    list.replaceChildren(...parts);
    if (focusedId) tiles.get(Number(focusedId))?.focus();
    syncSelection();
    void fetchIcons();
  }

  /** Rust проверяет видимые при поиске/фильтре строки первыми; прочие ждут. */
  function reportVisible(ids: number[]) {
    clearTimeout(visibleTimer);
    visibleTimer = setTimeout(() => {
      const key = ids.join(",");
      if (key === visibleSent) return;
      visibleSent = key;
      void call("owned_visible", { trackIds: ids }).catch(() => undefined);
    }, 300);
  }

  /** Одна строка после события `owned`: остальной список и фокус не трогаем. */
  function renderTile(id: number) {
    const old = tiles.get(id);
    const r = rows.find((x) => x.track_id === id);
    if (!old || !r) return;
    const had = document.activeElement === old;
    const t = tileEl(r);
    old.replaceWith(t);
    tiles.set(id, t);
    if (had) t.focus();
    if (id === curId) renderInspector();
  }

  function chosen(): RowDto[] {
    return rows.filter((r) => selected.has(r.track_id) && selectable(r));
  }

  function renderBar() {
    const items = chosen();
    const { count, size } = selectionSummary(items);
    const label = selectionLabel(count, hiddenCount(items, chip, query, withPhone));
    countEl.textContent = label;
    sizeEl.textContent = count && size ? size : "";
    sizeEl.hidden = !sizeEl.textContent;
    clearBtn.hidden = count === 0;
    const bar = barState(count, !!targetPhone(), toIphone());
    startBtn.disabled = bar.go.disabled;
    startBtn.textContent = bar.go.text;
    onlyBtn.disabled = bar.only.disabled;
    onlyBtn.textContent = bar.only.text;
    barHint.textContent = bar.hint ?? "";
    barHint.hidden = !bar.hint;
  }

  function clearAll() {
    for (const r of chosen()) setSelected(r.track_id, false);
    syncSelection();
  }

  function onOwned(ev: OwnedEvent) {
    if (!rows.some((r) => r.track_id === ev.track_id)) return;
    owned = applyOwned(owned, ev);
    rows = applyOwnedRow(rows, ev);
    if (!loaded) return;
    // ошибка «стоп» меняет все ждущие строки
    if (ev.error) renderList();
    else {
      renderTile(ev.track_id);
      renderChips();
      syncSelection();
    }
  }

  function onPurchasesProgress(p: PurchasesProgress) {
    progress = p;
    if (loaded && purchasesLoading) for (const el of progressEls) el.firstElementChild!.textContent = purchasesProgressText(p);
  }

  function onPurchases(d: RowsDto) {
    dto = d;
    // покупки построены для прежнего источника, а человек уже выбрал другой: группу перестроить
    const { rows: incoming, stale } = purchasesForSource(rows, d.rows, d.source_udid, chosenSource);
    if (!stale) {
      sourceUdid = d.source_udid;
      sourceError = null;
    }
    rows = mergePurchases(rows, incoming);
    for (const r of rows) if (!owned.has(r.track_id)) owned.set(r.track_id, initialOwned([r]).get(r.track_id)!);
    purchasesLoading = false;
    if (loaded) {
      renderChips();
      renderList();
    }
    // новые строки «С этого iPhone» проверяются вместе с ещё ждущими: новый вызов отменяет прежний
    if (stale && chosenSource) {
      // сбой перечитывания показываем, а не глотаем: иначе строки «С этого iPhone» молча старые
      void loadSource(chosenSource).catch((e) => {
        sourceBusy = false;
        sourceError = errorText(asUiError(e));
        renderChips();
        renderList();
      });
      return;
    }
    const phoneIds = idsToCheck(d.rows.filter((r) => r.source === "phone"));
    if (phoneIds.length) void call("check_owned", { trackIds: pendingChecks(rows, owned) }).catch(() => undefined);
  }

  async function recheck(id: number) {
    const ids = recheckIds(owned, id);
    owned = new Map(owned).set(id, { kind: "checking" });
    renderTile(id);
    syncSelection();
    await call("check_owned", { trackIds: ids });
  }

  function renderSweep() {
    sweepBtn.hidden = !showSweepButton(sweep, !!state.account);
    const text = sweepError ?? sweepText(sweep);
    sweepStatus.textContent = text ?? "";
    sweepStatus.hidden = !text;
    sweepStatus.classList.toggle("error", !!sweepError);
  }

  /** «Проверить весь каталог»: Rust ставит все записи без свежего ответа в конец очереди. */
  async function checkCatalog() {
    const seq = ++sweepSeq;
    sweepError = null;
    // ждущие — как были (новая очередь заменяет прежнюю); строки с ошибкой — вместе со всем каталогом
    const waiting = pendingChecks(rows, owned);
    owned = retryCatalogErrors(rows, owned);
    sweep = { checked: 0, total: 0, running: true };
    renderList();
    try {
      const p = await call<SweepDto>("check_catalog", { trackIds: waiting });
      if (seq === sweepSeq) sweep = p;
    } catch (e) {
      if (seq === sweepSeq) {
        sweep = null;
        sweepError = errorText(asUiError(e));
      }
    }
    renderSweep();
  }

  function onCatalogCheck(p: SweepDto) {
    sweepSeq++;
    sweep = p;
    sweepError = null;
    renderSweep();
  }

  /** Строка из `add_by_link`: наверх, права есть, отмечена. */
  function takeLinkRow(row: RowDto) {
    rows = addLinkRow(rows, row);
    owned = new Map(owned).set(row.track_id, { kind: "yes" });
    setSelected(row.track_id, true);
  }

  /** Несколько номеров (текст со ссылками, перетаскивание): каждый через `add_by_link` с проверкой
   * владения, по одному (`runBatch`). Итог — в `out`, рядом с ним «Отменить»; уход с «Выбора»
   * тоже останавливает. Пока идёт, новые номера ждут в `state.pendingLinks`. */
  async function addMany(ids: number[], out: HTMLElement) {
    if (linkBusy) {
      state.pendingLinks = mergePending(state.pendingLinks, ids);
      refreshSidebar();
      return;
    }
    linkBusy = true;
    batchStop = false;
    linkBtn.disabled = true;
    out.hidden = true;
    out.after(cancelBtn);
    cancelBtn.hidden = false;
    const r = await runBatch(
      ids,
      async (id) => {
        takeLinkRow(await call<RowDto>("add_by_link", { link: String(id) }));
        if (left) return;
        renderChips();
        renderList();
      },
      {
        cancelled: () => batchStop || left,
        step: (i, n) => (linkBtn.textContent = `Проверяем ${i} из ${n}…`),
        errorText: (e) => errorText(asUiError(e)),
      },
    );
    cancelBtn.hidden = true;
    out.textContent = batchText(r);
    out.hidden = false;
    linkBusy = false;
    linkBtn.disabled = false;
    linkBtn.textContent = "Добавить";
    if (!r.cancelled) takePending();
  }

  /** Номера, перетащенные до «Выбора» или пока шло добавление. */
  function takePending() {
    const next = state.pendingLinks;
    if (!next.length || linkBusy) return;
    state.pendingLinks = [];
    state.dropNote = null;
    refreshSidebar();
    void addMany(next, dropMsg);
  }

  /** Перетащили в окно: номера — по ссылкам сюда же; копии уже в «Мои приложения». */
  function onDrop(d: DropDto) {
    const parts = dropMessages(d);
    if (d.copies.length) parts.unshift(`Сохранено в «Мои приложения»: ${d.copies.map((c) => c.name).join(", ")}`);
    dropMsg.textContent = parts.join(". ");
    dropMsg.hidden = !parts.length;
    if (d.ids.length) void addMany(d.ids, dropMsg);
  }

  async function addLink(e: Event) {
    e.preventDefault();
    const link = linkInput.value.trim();
    if (!link || linkBusy) return;
    // несколько ссылок сразу — каждая по очереди
    const ids = await call<number[]>("links_in_text", { text: link }).catch(() => [] as number[]);
    if (ids.length > 1) {
      linkInput.value = "";
      return addMany(ids, linkMsg);
    }
    linkBusy = true;
    linkBtn.disabled = true;
    linkBtn.textContent = "Проверяем…";
    linkMsg.hidden = true;
    try {
      takeLinkRow(await call<RowDto>("add_by_link", { link }));
      linkInput.value = "";
      renderChips();
      renderList();
    } catch (err) {
      linkMsg.textContent = errorText(asUiError(err));
      linkMsg.hidden = false;
    }
    linkBusy = false;
    linkBtn.disabled = false;
    linkBtn.textContent = "Добавить";
    takePending();
  }

  /** «Вернуть» или «Только скачать» (`downloadOnly`: в «Мои приложения», на телефон не ставить).
   * «Вернуть»: последняя версия, которой нужна iOS новее телефона, только скачивается. */
  async function start(downloadOnly: boolean) {
    const items = chosen();
    if (items.length === 0) return;
    if (!downloadOnly && !targetPhone()) return;
    startBtn.disabled = true;
    onlyBtn.disabled = true;
    started = true;
    state.choice = null;
    const ios = phoneIos();
    const tooOld = (r: RowDto) => !state.versions.has(r.track_id) && tooOldNote(r.min_ios, ios) !== null;
    const split = downloadOnly ? { install: [], downloadOnly: items } : splitStart(items, tooOld);
    const batches: [RowDto[], boolean][] = [
      [split.install, false],
      [split.downloadOnly, true],
    ];
    for (const [batch, only] of batches) {
      if (!batch.length) continue;
      for (const r of batch) {
        state.jobs.set(r.track_id, { track_id: r.track_id, name: r.name, row: r, state: { kind: "queued" }, fromSaved: null, downloadOnly: only } as Job);
      }
      const ids = batch.map((r) => r.track_id);
      try {
        state.run = await call<RunDto>("start", { items: batch.map((r) => [r.track_id, r.name]), sameAppleId: state.sameAppleId ?? true, downloadOnly: only, versions: startVersions(ids, state.versions) });
      } catch {
        // start в Rust не падает; на всякий случай строки остаются «В очереди»
      }
    }
    // выбор версии — на один запуск: Rust помнит его для «Повторить»
    for (const r of items) state.versions.delete(r.track_id);
    go("done");
  }

  async function loadPurchases() {
    purchasesLoading = true;
    progress = null;
    if (dto) dto = { ...dto, purchases_error: null };
    if (loaded) renderList();
    await call("load_purchases");
  }

  async function load() {
    renderList();
    state.newIds = new Set(await call<number[]>("release_badges").catch(() => [] as number[]));
    const phone = targetPhone();
    if (state.withPhone && phone) {
      try {
        await call<InstalledDto[]>("installed_apps", { udid: phone.udid });
        withPhone = true;
      } catch (e) {
        installedError = asUiError(e);
        await call("clear_installed").catch(() => undefined);
      }
    } else {
      await call("clear_installed").catch(() => undefined);
    }
    try {
      const d = await call<RowsDto>("load_catalog_rows");
      // найденное в истории этим Apple ID раньше — снова на экране (и проверяется вместе со всем)
      rows = mergeHistoryRows(d.rows, keptHistory(state.history, state.account?.email ?? null));
    } catch {
      rows = [];
    }
    owned = initialOwned(rows);
    loaded = true;
    heading.textContent = withPhone && toIphone() ? "Что вернуть на iPhone" : "Что вернуть";
    renderPhoneCard();
    renderChips();
    renderList();
    const ids = idsToCheck(rows);
    if (ids.length) await call("check_owned", { trackIds: ids });
    await loadPurchases();
    takePending();
  }

  const heading = h("h2", { text: "Что вернуть" });
  const el = h(
    "div",
    { class: "body choose" },
    h(
      "div",
      { class: "choose-top" },
      h("div", { class: "choose-head" }, heading, h("label", { class: "searchbox" }, searchIcon(), search)),
      h("div", { class: "hintrow" }, found, findLink, switchId, h("span", { class: "hint search-hint", text: "Ищет везде, включая App Store, и по прежним названиям" })),
      resumeBanner(),
      dropMsg,
    ),
    h("div", { class: "choose-main" }, list, inspector),
    h(
      "div",
      { class: "selbar" },
      h("div", { class: "selinfo" }, countEl, h("div", { class: "selsub" }, sizeEl, clearBtn)),
      h("div", { class: "sp" }),
      h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("phone") } }),
      onlyBtn,
      startBtn,
    ),
    barHint,
  );
  void load().catch((e) => console.error(e));
  /** Подключили или отключили телефон: выбор источника над группой перерисовать. */
  function onPhones() {
    // «Вернуть» зависит от того, есть ли куда ставить: панель и карточка телефона — на каждое событие
    renderBar();
    renderPhoneCard();
    const key = state.phones.map((p) => p.udid).join(",");
    if (key === phonesKey) return;
    phonesKey = key;
    if (loaded && !purchasesLoading) renderList();
  }

  return {
    el,
    onOwned,
    onPhones,
    onPurchasesProgress,
    onPurchases,
    onCatalogCheck,
    onDrop,
    onNewReleases: () => {
      if (loaded) renderList();
    },
    onHistoryProgress,
    onHistory,
    // Уход с «Выбора» останавливает проверку прав: пустой список отменяет идущую.
    leave: () => {
      left = true;
      clearTimeout(visibleTimer);
      for (const t of waitTimers.values()) clearInterval(t);
      if (!started) state.choice = { selected: [...selected] };
      void call("check_owned", { trackIds: [] }).catch(() => undefined);
    },
  };
});
