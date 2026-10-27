// «Все копии»: любая ipa из «Мои приложения» (всех Apple ID) или выбранный файл — на любой
// подключённый телефон. Работает без входа. Диалог и файлы — только в Rust.
import { asUiError, call, go, register, safe, state, type Job } from "../app";
import { copyInstallButton, copyNotes, copyPhone, copyRow, copyWarning, groupCopies, markCopyPhone } from "../copies";
import { h } from "../dom";
import { jobInbox } from "../jobs";
import { jobView, errorText } from "../labels";
import type { AccountMatchDto, CopyDto } from "../types";
import { appIcon } from "./choose";
import { jobRow, phoneStatus, refreshPhones, retryJob } from "./phone";

register("copies", () => {
  let copies: CopyDto[] | null = null;
  let query = "";
  let selected: string | null = null;
  /** Итог `copy_account_match` для пары (копия, телефон). */
  let match: { key: string; m: AccountMatchDto } | null = null;
  /** Задания, запущенные с этого экрана. */
  const mine: number[] = [];

  const list = h("div", { class: "scroll" });
  const msg = h("p", { class: "error", attrs: { role: "alert", "aria-live": "polite" } });
  msg.hidden = true;
  const warning = h("div", { attrs: { "aria-live": "polite" } });
  const jobsBox = h("div");
  const installBtn = h("button", { class: "btn", on: { click: safe(install) } });
  /** Свой телефон экрана: установка копии не меняет, куда ставит главный поток. */
  let chosenPhone: string | null = null;
  const targetPhone = () => copyPhone(state.phones, chosenPhone);
  const status = phoneStatus(() => onPhones(), {
    phones: () => markCopyPhone(state.phones, chosenPhone),
    choose: (udid) => {
      chosenPhone = udid;
    },
  });

  const search = h("input", {
    class: "input search",
    attrs: { type: "search", placeholder: "Найти копию по названию", "aria-label": "Найти копию", spellcheck: "false" },
    on: {
      input: () => {
        query = search.value;
        renderList();
      },
    },
  });
  const pickBtn = h("button", {
    class: "btn ghost small",
    text: "Выбрать файл .ipa…",
    attrs: { type: "button" },
    on: { click: safe(pickFile) },
  });

  const current = () => copies?.find((c) => c.path === selected) ?? null;
  const pairKey = () => {
    const c = current();
    const t = targetPhone();
    return c && t ? `${t.udid}\n${c.path}` : null;
  };

  async function load(): Promise<void> {
    try {
      copies = await call<CopyDto[]>("all_copies");
    } catch (e) {
      copies = [];
      say(errorText(asUiError(e)));
    }
    renderList();
    renderAction();
  }

  function say(text: string | null): void {
    msg.textContent = text ?? "";
    msg.hidden = !text;
  }

  async function pickFile(): Promise<void> {
    say(null);
    try {
      const c = await call<CopyDto | null>("pick_ipa");
      if (!c) return;
      if (copies && !copies.some((x) => x.path === c.path)) copies.push(c);
      selected = c.path;
      query = "";
      search.value = "";
      renderList();
      onSelect();
    } catch (e) {
      say(errorText(asUiError(e)));
    }
  }

  async function checkMatch(): Promise<void> {
    const key = pairKey();
    const c = current();
    const t = targetPhone();
    if (!key || !c || !t || match?.key === key) return;
    let m: AccountMatchDto = { kind: "unknown" };
    try {
      m = await call<AccountMatchDto>("copy_account_match", { path: c.path, udid: t.udid });
    } catch {
      // не узнали — без предупреждения
    }
    if (pairKey() === key) {
      match = { key, m };
      renderAction();
    }
  }

  function onSelect(): void {
    renderAction();
    void checkMatch();
  }

  function onPhones(): void {
    status.render();
    renderAction();
    void checkMatch();
  }

  async function install(): Promise<void> {
    const c = current();
    const t = targetPhone();
    if (!c || !t || copyInstallButton(c, t).disabled) return;
    say(null);
    installBtn.disabled = true;
    try {
      const id = await call<number>("install_copy", { path: c.path, udid: t.udid });
      const job: Job = { track_id: id, name: c.name, row: copyRow(c), state: { kind: "queued" }, fromSaved: null };
      // события, пришедшие до ответа install_copy (быстрый отказ телефона), не теряются
      jobInbox.register(state.jobs, job);
      if (!mine.includes(id)) mine.push(id);
      renderJobs();
    } catch (e) {
      say(errorText(asUiError(e)));
    }
    renderAction();
  }

  function rowEl(c: CopyDto): HTMLElement {
    const radio = h("input", { class: "box", attrs: { type: "radio", name: "copy", "aria-label": c.name } });
    radio.checked = c.path === selected;
    radio.addEventListener("change", () => {
      if (!radio.checked) return;
      selected = c.path;
      onSelect();
    });
    const notes = copyNotes(c).map((t) => h("small", { class: "muted", text: t }));
    return h("label", { class: "app" }, radio, appIcon(copyRow(c)), h("span", { class: "t" }, h("b", { text: c.name }), ...notes));
  }

  function renderList(): void {
    if (copies === null) {
      list.replaceChildren(h("p", { class: "loading" }, "Открываем…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
      return;
    }
    if (!copies.length) {
      list.replaceChildren(h("p", { class: "empty", text: "Копий пока нет" }));
      return;
    }
    const groups = groupCopies(copies, query);
    if (!groups.length) {
      list.replaceChildren(h("p", { class: "empty", text: "Ничего не нашлось." }));
      return;
    }
    list.replaceChildren(...groups.flatMap((g) => [h("h3", { class: "group", text: g.title }), ...g.copies.map(rowEl)]));
  }

  function renderAction(): void {
    const c = current();
    const b = copyInstallButton(c, targetPhone());
    installBtn.textContent = b.text;
    installBtn.disabled = b.disabled;
    const key = pairKey();
    const text = c && key && match?.key === key ? copyWarning(c, match.m) : null;
    warning.replaceChildren(...(text ? [h("div", { class: "banner" }, h("span", { text }))] : []));
  }

  function renderJobs(): void {
    const jobs = mine.map((id) => state.jobs.get(id)).filter((j): j is Job => !!j);
    jobsBox.replaceChildren(
      ...jobs.map((j) =>
        jobRow(
          j,
          jobView(j.state, j.fromSaved).text,
          safe(async () => {
            j.state = { kind: "queued" };
            renderJobs();
            await retryJob(j.track_id);
          }),
        ),
      ),
    );
  }

  renderList();
  renderAction();
  void load();
  void refreshPhones().then(onPhones);
  const back = h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go(state.beforeCopies) } });
  const el = h(
    "div",
    { class: "body" },
    h("h2", { text: "Все копии" }),
    h("p", { class: "lead", text: "Любую копию можно поставить на любой подключённый телефон." }),
    status.el,
    h("div", { class: "addlink" }, search, pickBtn),
    msg,
    list,
    warning,
    jobsBox,
    h("div", { class: "push" }, installBtn, back),
  );
  return { el, onJob: renderJobs, onPhones };
});
