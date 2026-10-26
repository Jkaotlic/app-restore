import { asUiError, call, go, register, safe, state, targetIsIphone, targetPhone, type Job } from "../app";
import { h } from "../dom";
import { installCaption, purchasesProgressText, unknownAccountNote, versionLine } from "../labels";
import { returnLabel } from "../plural";
import {
  addLinkRow,
  applyOwned,
  applyOwnedRow,
  canSelect,
  groupRows,
  iconFor,
  idsToCheck,
  initialOwned,
  mergePurchases,
  ownedView,
  recheckIds,
  type OwnedMap,
} from "../rows";
import type { InstalledDto, OwnedEvent, PurchasesProgress, RowDto, RowsDto, RunDto, UiError } from "../types";

export function appIcon(r: RowDto): HTMLElement {
  const { letter, color } = iconFor(r);
  return h("span", { class: [...letter].length > 2 ? "ico tight" : "ico", text: letter, style: { background: color } });
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
  let loaded = false;
  /** Покупки ещё грузятся; `progress` — последняя страница. */
  let purchasesLoading = true;
  let progress: PurchasesProgress | null = null;
  let dto: RowsDto | null = null;
  let installedError: UiError | null = null;
  /** Колонка «На iPhone»: телефон выбран и список с него прочитан. */
  let withPhone = false;
  // «Только для iPad» зависит от устройства, на которое ставим, а не от «Дальше» (как и `checked` в Rust)
  const toIphone = () => targetIsIphone();

  const list = h("div", { class: "scroll" });
  const banners = h("div", { attrs: { "aria-live": "polite" } });
  const startBtn = h("button", { class: "btn", on: { click: safe(start) } });

  const search = h("input", {
    class: "input search",
    attrs: { type: "search", placeholder: "Найти другое приложение по названию", "aria-label": "Найти в покупках", spellcheck: "false" },
    on: {
      input: () => {
        query = search.value;
        renderOther();
      },
    },
  });
  const otherBox = h("div", { attrs: { "aria-live": "polite" } });
  const removedBox = h("div");

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
    h("p", { class: "note", text: "Добавить по ссылке App Store" }),
    h("div", { class: "addlink" }, linkInput, linkBtn),
    linkMsg,
  );

  const switchId = h("button", {
    class: "link",
    text: "Войти в другой Apple ID",
    on: {
      // список Apple ID: сохранённый вход подставится без пароля; при смене аккаунта галочки сбросятся
      click: () => go("accounts"),
    },
  });

  const selectable = (r: RowDto) => canSelect(r, owned.get(r.track_id), toIphone());

  function rowEl(r: RowDto): HTMLElement {
    const s = owned.get(r.track_id);
    const enabled = selectable(r);
    const box = h("input", { class: "box", attrs: { type: "checkbox", "aria-label": r.name } });
    box.checked = enabled && selected.has(r.track_id);
    box.disabled = !enabled;
    box.addEventListener("change", () => {
      if (box.checked) {
        selected.add(r.track_id);
        picked.add(r.track_id);
        unchecked.delete(r.track_id);
      } else {
        selected.delete(r.track_id);
        picked.delete(r.track_id);
        unchecked.add(r.track_id);
      }
      renderButton();
    });

    const lines: (HTMLElement | null)[] = [];
    if (r.source === "removed" && s) {
      const v = ownedView(s);
      const small = h("small", { class: s.kind === "yes" ? "" : "muted", text: v.text });
      if (v.spinner) small.append(h("span", { class: "spin", attrs: { "aria-hidden": "true" } }));
      if (v.recheck) {
        small.append(" ");
        small.append(
          h("button", {
            class: "link",
            text: "Повторить проверку",
            attrs: { type: "button" },
            on: { click: safe((e: Event) => recheck(e, r.track_id)) },
          }),
        );
      }
      lines.push(small);
    } else if (r.source === "link") {
      lines.push(h("small", { text: "Есть в ваших покупках" }));
    } else if (r.removed_from_ru) {
      lines.push(h("small", { class: "muted hint", text: "нет в российском App Store" }));
    }
    if (r.ipad_only) lines.push(h("small", { class: "muted", text: "Только для iPad" }));
    const caption = installCaption(r.install_state, withPhone);
    if (caption) lines.push(h("small", { class: r.install_state === "update_available" ? "" : "muted", text: caption }));
    const versions = versionLine(r, withPhone);
    if (versions) lines.push(h("small", { class: "muted", text: versions }));
    if (r.has_saved_copy) lines.push(h("small", { class: "muted", text: "Есть копия на компьютере" }));
    const unknownId = unknownAccountNote(r);
    if (unknownId) lines.push(h("small", { class: "muted", text: unknownId }));

    return h(
      "label",
      { class: enabled ? "app" : "app no" },
      box,
      appIcon(r),
      h("span", { class: "t" }, h("b", { text: r.name }), ...lines),
    );
  }

  function renderOther() {
    const { other } = groupRows(rows, query);
    const purchases = rows.filter((r) => r.source === "purchase" || r.source === "saved");
    const parts: HTMLElement[] = [];
    if (purchasesLoading) {
      parts.push(h("p", { class: "loading" }, purchasesProgressText(progress), h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
    } else if (purchases.length === 0 && !dto?.purchases_error) {
      parts.push(h("p", { class: "empty", text: "На этом Apple ID нет покупок для iPhone. Проверьте, тот ли это Apple ID." }));
    }
    search.hidden = purchasesLoading || purchases.length === 0;
    if (other.length) parts.push(...other.map(rowEl));
    else if (query && !purchasesLoading) parts.push(h("p", { class: "empty", text: "Ничего не нашлось." }));
    otherBox.replaceChildren(...parts);
  }

  function renderBanners() {
    const parts: HTMLElement[] = [];
    if (installedError) parts.push(h("p", { class: "note", text: "Не удалось прочитать, что стоит на iPhone" }));
    const pe = dto?.purchases_error;
    if (pe) {
      const b = h("div", { class: "banner" }, h("span", { text: pe.text }));
      if (pe.code === "not_logged_in" || pe.code === "apple_changed_login") {
        b.append(h("button", { class: "btn ghost small", text: "Войти снова", on: { click: () => go("login") } }));
      } else {
        b.append(h("button", { class: "btn ghost small", text: "Загрузить снова", on: { click: safe(loadPurchases) } }));
      }
      parts.push(b);
    }
    if (dto?.ru_check_failed) parts.push(h("p", { class: "note", text: "Не удалось проверить российский App Store" }));
    banners.replaceChildren(...parts);
  }

  function render() {
    if (!loaded) {
      list.replaceChildren(h("p", { class: "loading" }, "Открываем…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
      renderButton();
      return;
    }
    renderBanners();
    renderRemoved();
    renderOther();
    list.replaceChildren(
      banners,
      h("h3", { class: "group", text: "Пропали из App Store" }),
      removedBox,
      linkForm,
      h("h3", { class: "group", text: "Остальные покупки" }),
      search,
      otherBox,
    );
    renderButton();
  }

  /** Только группа каталога: поиск и поле ссылки остаются на месте и не теряют фокус. */
  function renderRemoved() {
    const { removed } = groupRows(rows, "");
    removedBox.replaceChildren(
      ...(removed.length ? removed.map(rowEl) : [h("p", { class: "empty", text: "Пока пусто. Добавьте приложение по ссылке ниже." })]),
    );
    renderButton();
  }

  function chosen(): RowDto[] {
    return rows.filter((r) => selected.has(r.track_id) && selectable(r));
  }

  function renderButton() {
    const n = chosen().length;
    startBtn.textContent = returnLabel(n);
    startBtn.disabled = n === 0;
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
    if (!ev.error) autoSelect(ev.track_id, ev.checked);
    if (loaded) renderRemoved();
  }

  function onPurchasesProgress(p: PurchasesProgress) {
    progress = p;
    if (loaded && purchasesLoading) renderOther();
  }

  function onPurchases(d: RowsDto) {
    dto = d;
    rows = mergePurchases(rows, d.rows);
    for (const r of rows) if (!owned.has(r.track_id)) owned.set(r.track_id, initialOwned([r]).get(r.track_id)!);
    purchasesLoading = false;
    if (loaded) render();
  }

  async function recheck(e: Event, id: number) {
    e.preventDefault();
    e.stopPropagation();
    const ids = recheckIds(owned, id);
    owned = new Map(owned).set(id, { kind: "checking" });
    renderRemoved();
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
      selected.add(row.track_id);
      picked.add(row.track_id);
      unchecked.delete(row.track_id);
      linkInput.value = "";
      renderRemoved();
      renderOther();
    } catch (err) {
      linkMsg.textContent = asUiError(err).text;
      linkMsg.hidden = false;
    }
    linkBtn.disabled = false;
    linkBtn.textContent = "Добавить";
  }

  async function start() {
    const items = chosen();
    if (items.length === 0) return;
    startBtn.disabled = true;
    started = true;
    state.choice = null;
    for (const r of items) {
      state.jobs.set(r.track_id, { track_id: r.track_id, name: r.name, row: r, state: { kind: "queued" }, fromSaved: null } as Job);
    }
    try {
      state.run = await call<RunDto>("start", { items: items.map((r) => [r.track_id, r.name]), sameAppleId: state.sameAppleId ?? true });
    } catch {
      // start в Rust не падает; на всякий случай строки остаются «В очереди»
    }
    go("done");
  }

  async function loadPurchases() {
    purchasesLoading = true;
    progress = null;
    if (dto) dto = { ...dto, purchases_error: null };
    if (loaded) render();
    await call("load_purchases");
  }

  async function load() {
    render();
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
    render();
    const ids = idsToCheck(rows);
    if (ids.length) await call("check_owned", { trackIds: ids });
    await loadPurchases();
  }

  const el = h(
    "div",
    { class: "body" },
    h("h2", { text: "Что вернуть" }),
    h("p", { class: "lead", text: "Ушли из российского App Store. Проверили по вашим покупкам." }),
    list,
    h("div", { class: "push" }, startBtn, h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("phone") } }), switchId),
  );
  void load().catch((e) => console.error(e));
  return {
    el,
    onOwned,
    onPurchasesProgress,
    onPurchases,
    // Уход с «Выбора» останавливает проверку прав: пустой список отменяет идущую.
    leave: () => {
      if (!started) state.choice = { selected: [...selected], unchecked: [...unchecked], picked: [...picked] };
      void call("check_owned", { trackIds: [] }).catch(() => undefined);
    },
  };
});
