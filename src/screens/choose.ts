import { asUiError, call, go, register, safe, state, targetIsIphone, targetPhone, type Job } from "../app";
import { h } from "../dom";
import { purchasesProgressText, errorText } from "../labels";
import {
  addLinkRow,
  applyOwned,
  applyOwnedRow,
  autoChecks,
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
  rovingIndex,
  hiddenCount,
  selectionLabel,
  selectionSummary,
  tileA11y,
  tileLine,
  visibleGroups,
  type Chip,
  type GroupKey,
  type OwnedMap,
  type TileLine,
} from "../rows";
import type { InstalledDto, OwnedEvent, PurchasesProgress, RowDto, RowsDto, RunDto, SourceRowsDto, UiError } from "../types";

export function appIcon(r: RowDto): HTMLElement {
  const { letter, color } = iconFor(r);
  const cls = ["ico", [...letter].length > 2 ? "tight" : "", isLight(color) ? "light" : ""].filter(Boolean).join(" ");
  return h("span", { class: cls, text: letter, style: { background: color }, attrs: { "aria-hidden": "true" } });
}

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
  // «Назад» на шаг iPhone и обратно не сбрасывает галочки
  const saved = state.choice;
  const selected = new Set<number>(saved?.selected);
  /** Человек сам снял галочку: автоматически её не возвращаем. */
  const unchecked = new Set<number>(saved?.unchecked);
  /** Человек сам поставил галочку: автоматически её не снимаем. */
  const picked = new Set<number>(saved?.picked);
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
  let storeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Подключённые телефоны (udid через запятую): выбор источника перерисовывается, только когда они меняются. */
  let phonesKey = state.phones.map((p) => p.udid).join(",");
  // «Только для iPad» зависит от устройства, на которое ставим, а не от «Дальше» (как и `checked` в Rust)
  const toIphone = () => targetIsIphone();

  /** Плитки на экране по trackId и кнопки «Выбрать все в группе»: выбор меняется на месте. */
  const tiles = new Map<number, HTMLButtonElement>();
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
  const found = h("p", { class: "found", attrs: { role: "status" } });
  const chips = h("div", { class: "chips", attrs: { role: "toolbar", "aria-label": "Фильтры" } });
  chips.addEventListener("keydown", (e: KeyboardEvent) => {
    const all = [...chips.querySelectorAll<HTMLButtonElement>(".chip")];
    const i = rovingIndex(e.key, all.indexOf(document.activeElement as HTMLButtonElement), all.length);
    if (i === null || !all.length) return;
    e.preventDefault();
    for (const b of all) b.tabIndex = -1;
    all[i].tabIndex = 0;
    all[i].focus();
    all[i].scrollIntoView({ block: "nearest", inline: "nearest" });
  });

  const search = h("input", {
    class: "input",
    attrs: {
      type: "search",
      placeholder: "Название, например «Сбер» или «ВК»",
      "aria-label": "Поиск по каталогу, покупкам и телефону",
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
    { class: "linkform", attrs: { novalidate: true }, on: { submit: safe(addLink) } },
    h("p", { class: "note", text: "Нет в списке? Добавьте по ссылке App Store" }),
    h("div", { class: "addlink" }, linkInput, linkBtn),
    linkMsg,
  );

  const switchId = h("button", {
    class: "link",
    text: "Войти в другой Apple ID",
    attrs: { type: "button" },
    on: {
      // список Apple ID: сохранённый вход подставится без пароля; при смене аккаунта галочки сбросятся
      click: () => go("accounts"),
    },
  });

  const selectable = (r: RowDto) => canSelect(r, owned.get(r.track_id), toIphone());

  function setSelected(id: number, on: boolean) {
    if (on) {
      selected.add(id);
      picked.add(id);
      unchecked.delete(id);
    } else {
      selected.delete(id);
      picked.delete(id);
      unchecked.add(id);
    }
  }

  function tileEl(r: RowDto): HTMLButtonElement {
    const line = tileLine(r, owned.get(r.track_id), toIphone(), withPhone);
    const enabled = selectable(r);
    const ver = h("span", { class: `ver ${line.tone}`, text: line.text });
    if (line.up) ver.append(h("span", { class: "up", text: line.up }));
    if (line.tone === "checking") ver.append(h("span", { class: "spin", attrs: { "aria-hidden": "true" } }));
    const tile = h(
      "button",
      {
        class: "tile",
        attrs: { type: "button", title: `${r.name} — ${line.text}${line.up ?? ""}${line.note ? ` · ${line.note}` : ""}` },
        on: {
          // `aria-disabled`, не `disabled`: плитка в порядке Tab и называет причину; нажатие ничего не делает
          click: safe(async () => {
            if (line.tone === "retry") return recheck(r.track_id);
            if (!selectable(r)) return;
            setSelected(r.track_id, !selected.has(r.track_id));
            syncSelection();
          }),
        },
      },
      appIcon(r),
      h("span", { class: "tt" }, h("span", { class: "nm", text: r.name }), ver, line.note ? h("span", { class: "ver note", text: line.note }) : null),
      h("span", { class: "tbox", attrs: { "aria-hidden": "true" } }),
    );
    tile.dataset.id = String(r.track_id);
    applyA11y(tile, r, line, enabled);
    return tile;
  }

  /** Плитка группы «С этого iPhone» и её кнопки под ней: внутри кнопки-плитки другие кнопки нельзя. */
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
      for (const r of rows) if (!owned.has(r.track_id)) owned.set(r.track_id, initialOwned([r]).get(r.track_id)!);
      storeStatus = "idle";
      // новый вызов отменяет прежний: отправляем всё, что ещё ждёт проверки
      const pending = pendingChecks(rows, owned);
      if (pending.length) void call("check_owned", { trackIds: pending }).catch(() => undefined);
    } catch {
      if (req !== storeReq) return;
      storeStatus = "error";
    }
    if (!loaded) return;
    renderChips();
    renderList();
  }

  function applyA11y(tile: HTMLElement, r: RowDto, line: TileLine, enabled: boolean) {
    const a = tileA11y(r.name, line, enabled, selected.has(r.track_id));
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
      if (r) applyA11y(tile, r, tileLine(r, owned.get(id), toIphone(), withPhone), selectable(r));
    }
    for (const g of groupLinks) {
      const pick = groupPick(g.rows, selectable, selected);
      g.btn.hidden = pick.ids.length === 0;
      g.btn.textContent = pick.allOn ? "Снять в группе" : "Выбрать все в группе";
    }
    renderBar();
  }

  function renderChips() {
    const counts = chipCounts(rows, query, withPhone);
    const keys = chipList(withPhone);
    if (!keys.some((c) => c.key === chip)) chip = "all";
    const focused = (document.activeElement as HTMLElement | null)?.dataset?.chip;
    chips.replaceChildren(
      ...keys.map((c) => {
        const b = h(
          "button",
          {
            class: "chip",
            attrs: { type: "button", "aria-pressed": String(c.key === chip) },
            on: {
              click: () => {
                chip = c.key;
                renderChips();
                renderList();
              },
            },
          },
          c.label,
          h("span", { text: String(counts[c.key]) }),
        );
        b.dataset.chip = c.key;
        // в Tab один чип (выбранный), по остальным — стрелками
        b.tabIndex = c.key === chip ? 0 : -1;
        return b;
      }),
    );
    if (focused) chips.querySelector<HTMLElement>(`[data-chip="${focused}"]`)?.focus();
  }

  function groupSection(key: GroupKey, title: string, items: RowDto[], extra: HTMLElement[], after: HTMLElement[] = []): HTMLElement {
    const btn = h("button", { class: "link", attrs: { type: "button" } });
    btn.addEventListener("click", () => {
      const pick = groupPick(items, selectable, selected);
      for (const id of pick.ids) setSelected(id, !pick.allOn);
      syncSelection();
    });
    groupLinks.push({ key, rows: items, btn });
    const grid = h("div", { class: "tiles" });
    for (const r of items) {
      const t = tileEl(r);
      tiles.set(r.track_id, t);
      grid.append(r.source === "phone" ? phoneTile(r, t) : t);
    }
    return h(
      "section",
      { class: "gsec", attrs: { "aria-label": title } },
      h("div", { class: "ghead" }, h("h3", {}, title, h("small", { text: String(items.length) })), btn),
      ...extra,
      items.length ? grid : null,
      ...after,
    );
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
    return parts;
  }

  function renderList() {
    tiles.clear();
    groupLinks.length = 0;
    progressEls.length = 0;
    if (!loaded) {
      list.replaceChildren(h("p", { class: "loading" }, "Открываем…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
      renderBar();
      return;
    }
    const focusedId = (document.activeElement as HTMLElement | null)?.closest?.<HTMLElement>(".tile")?.dataset.id;
    const groups = visibleGroups(rows, chip, query, withPhone);
    found.textContent = foundText(rows, chip, query, withPhone);
    const plain = chip === "all" && query.trim() === "";
    const parts: HTMLElement[] = banners();
    for (const g of groups) {
      const extra: HTMLElement[] = [];
      if (g.key === "catalog" && !g.rows.length && plain) extra.push(h("p", { class: "empty", text: "Пока пусто. Добавьте приложение по ссылке ниже." }));
      if (g.key === "buy" && plain) {
        const purchases = rows.filter((r) => r.source === "purchase" || r.source === "saved");
        if (purchasesLoading) {
          extra.push(progressEl());
        } else if (purchases.length === 0 && !dto?.purchases_error) {
          extra.push(h("p", { class: "empty", text: "На этом Apple ID нет покупок для iPhone. Проверьте, тот ли это Apple ID." }));
        }
      }
      const after: HTMLElement[] = [];
      if (g.key === "store") {
        if (storeStatus === "loading") extra.push(h("p", { class: "loading" }, "Ищем в App Store…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
        else if (storeStatus === "error") extra.push(h("p", { class: "note", text: "App Store не ответил" }));
      }
      if (g.key === "phone" && !purchasesLoading) {
        const choices = sourceChoices(state.phones, sourceUdid);
        if (choices && (plain || g.rows.length)) extra.push(sourcePicker(choices));
        if (choices && plain && !g.rows.length) extra.push(h("p", { class: "empty", text: "На этом телефоне нет приложений, которых нет в каталоге и в российском App Store." }));
        if (g.rows.length) after.push(h("p", { class: "note group-hint", text: SOURCE_HINT }));
      }
      if (g.rows.length || extra.length) parts.push(groupSection(g.key, g.title, g.rows, extra, after));
    }
    if (groups.every((g) => !g.rows.length) && !plain) {
      parts.push(h("p", { class: "empty", text: query.trim() ? emptySearchText(query) : "В этом фильтре пусто." }));
      if (purchasesLoading) parts.push(progressEl());
    }
    parts.push(linkForm);
    list.replaceChildren(...parts);
    if (focusedId) tiles.get(Number(focusedId))?.focus();
    syncSelection();
  }

  /** Одна плитка после события `owned`: остальной список и фокус не трогаем. */
  function renderTile(id: number) {
    const old = tiles.get(id);
    const r = rows.find((x) => x.track_id === id);
    if (!old || !r) return;
    const had = document.activeElement === old;
    const t = tileEl(r);
    old.replaceWith(t);
    tiles.set(id, t);
    if (had) t.focus();
  }

  function chosen(): RowDto[] {
    return rows.filter((r) => selected.has(r.track_id) && selectable(r));
  }

  function renderBar() {
    const items = chosen();
    const { count, size } = selectionSummary(items);
    const label = selectionLabel(count, hiddenCount(items, chip, query, withPhone));
    countEl.replaceChildren(label, ...(count && size ? [h("small", { text: size })] : []));
    clearBtn.hidden = count === 0;
    const bar = barState(count, !!targetPhone());
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

  /** Отметка по умолчанию из Rust (`checked`); ручной выбор человека главнее. */
  function autoSelect(id: number, checked: boolean) {
    if (checked && !unchecked.has(id)) selected.add(id);
    else if (!checked && !picked.has(id)) selected.delete(id);
  }

  function onOwned(ev: OwnedEvent) {
    if (!rows.some((r) => r.track_id === ev.track_id)) return;
    owned = applyOwned(owned, ev);
    rows = applyOwnedRow(rows, ev);
    const r = rows.find((x) => x.track_id === ev.track_id);
    if (!ev.error && r && autoChecks(r)) autoSelect(ev.track_id, ev.checked);
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

  async function addLink(e: Event) {
    e.preventDefault();
    const link = linkInput.value.trim();
    if (!link) return;
    linkBtn.disabled = true;
    linkBtn.textContent = "Проверяем…";
    linkMsg.hidden = true;
    try {
      const row = await call<RowDto>("add_by_link", { link });
      rows = addLinkRow(rows, row);
      owned = new Map(owned).set(row.track_id, { kind: "yes" });
      setSelected(row.track_id, true);
      linkInput.value = "";
      renderChips();
      renderList();
    } catch (err) {
      linkMsg.textContent = errorText(asUiError(err));
      linkMsg.hidden = false;
    }
    linkBtn.disabled = false;
    linkBtn.textContent = "Добавить";
  }

  /** «Вернуть» или «Только скачать» (`downloadOnly`: в «Мои приложения», на телефон не ставить). */
  async function start(downloadOnly: boolean) {
    const items = chosen();
    if (items.length === 0) return;
    if (!downloadOnly && !targetPhone()) return;
    startBtn.disabled = true;
    onlyBtn.disabled = true;
    started = true;
    state.choice = null;
    for (const r of items) {
      state.jobs.set(r.track_id, { track_id: r.track_id, name: r.name, row: r, state: { kind: "queued" }, fromSaved: null, downloadOnly } as Job);
    }
    try {
      state.run = await call<RunDto>("start", { items: items.map((r) => [r.track_id, r.name]), sameAppleId: state.sameAppleId ?? true, downloadOnly });
    } catch {
      // start в Rust не падает; на всякий случай строки остаются «В очереди»
    }
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
      rows = d.rows;
    } catch {
      rows = [];
    }
    owned = initialOwned(rows);
    for (const r of rows) if (r.checked && selectable(r) && !unchecked.has(r.track_id)) selected.add(r.track_id);
    loaded = true;
    heading.textContent = withPhone && toIphone() ? "Что вернуть на iPhone" : "Что вернуть";
    renderChips();
    renderList();
    const ids = idsToCheck(rows);
    if (ids.length) await call("check_owned", { trackIds: ids });
    await loadPurchases();
  }

  const heading = h("h2", { text: "Что вернуть" });
  const el = h(
    "div",
    { class: "body choose" },
    h(
      "div",
      { class: "choose-top" },
      heading,
      h("label", { class: "searchbox" }, searchIcon(), search),
      h("p", { class: "hint", text: "Ищет в каталоге, ваших покупках, на телефоне и в App Store, в том числе по прежним названиям." }),
    ),
    chips,
    found,
    list,
    h(
      "div",
      { class: "selbar" },
      countEl,
      clearBtn,
      h("div", { class: "sp" }),
      h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("phone") } }),
      switchId,
      onlyBtn,
      startBtn,
    ),
    barHint,
  );
  void load().catch((e) => console.error(e));
  /** Подключили или отключили телефон: выбор источника над группой перерисовать. */
  function onPhones() {
    // «Вернуть» зависит от того, есть ли куда ставить: панель — на каждое событие
    renderBar();
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
    // Уход с «Выбора» останавливает проверку прав: пустой список отменяет идущую.
    leave: () => {
      if (!started) state.choice = { selected: [...selected], unchecked: [...unchecked], picked: [...picked] };
      void call("check_owned", { trackIds: [] }).catch(() => undefined);
    },
  };
});
