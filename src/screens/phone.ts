import { openUrl } from "@tauri-apps/plugin-opener";
import { asUiError, call, go, refreshSidebar, register, safe, state, targetPhone, type Job } from "../app";
import { h, ipadIcon, phoneIcon } from "../dom";
import { staysOnRefusal } from "../copies";
import { accountBlock, canProceed, chooserNote, jobView, otherIdText, phoneView, sameFromMatch, showSkip } from "../labels";
import type { AccountMatchDto, PhoneDto, SwitchDto } from "../types";
import { appIcon } from "./choose";

/** «Apple Devices» в Microsoft Store. Та же ссылка разрешена в capabilities/default.json. */
export const APPLE_DEVICES_URL = "https://apps.microsoft.com/detail/9np83lwlpz9k";

const deviceIcon = (p: PhoneDto) => (phoneView(p).isIpad ? ipadIcon() : phoneIcon());

/** Строка задания на «Готово»: полоса под названием, состояние и «Повторить» справа. */
export function jobRow(job: Job, text: string, onRetry: (e: Event) => void): HTMLElement {
  const v = jobView(job.state, job.fromSaved);
  const failed = job.state.kind === "failed";
  const retry = v.retry ? h("button", { class: "btn ghost small", text: "Повторить", on: { click: onRetry } }) : null;
  let bar: HTMLElement | null = null;
  if (v.bar !== null) {
    bar =
      v.bar === "indeterminate"
        ? h("div", { class: "bar indeterminate" }, h("i"))
        : h("div", { class: "bar" }, h("i", { style: { width: `${v.bar}%` } }));
  }
  const cls = failed ? "state bad" : job.state.kind === "installed" ? "state ok" : "state";
  const right = h("span", { class: "right" }, h("span", { class: cls, text }), retry);
  return h("div", { class: "app" }, appIcon(job.row), h("span", { class: "t" }, h("b", { text: job.name }), bar), right);
}

export async function retryJob(id: number): Promise<void> {
  await call("retry", { trackId: id });
}

export async function refreshPhones(): Promise<void> {
  try {
    state.phones = await call<PhoneDto[]>("list_phones");
    state.phonesError = null;
  } catch (e) {
    state.phonesError = asUiError(e);
  }
}

/** Что с телефоном: карточка, выбор из нескольких или «не подключён». Общая для «iPhone» и «Готово». */
export function phoneStatus(onChange: () => void): { el: HTMLElement; render(): void } {
  const el = h("div", { attrs: { "aria-live": "polite" } });

  function deviceCard(p: PhoneDto): HTMLElement {
    const v = phoneView(p);
    return h(
      "button",
      {
        class: p.target ? "device on" : "device",
        attrs: { type: "button", "aria-pressed": p.target ? "true" : "false" },
        on: {
          click: safe(async () => {
            await call("choose_phone", { udid: p.udid });
            await refreshPhones();
            render();
            onChange();
          }),
        },
      },
      deviceIcon(p),
      h("span", { class: "t" }, h("b", { text: v.title }), h("br"), h("span", { class: "note", text: v.subtitle })),
      p.target ? h("span", { class: "state", text: "Ставим сюда" }) : null,
    );
  }

  function render() {
    const phones = state.phones;
    const err = state.phonesError;
    const parts: HTMLElement[] = [];
    const note = chooserNote(phones);
    if (phones.length === 1 && phones[0].target) {
      const v = phoneView(phones[0]);
      parts.push(
        h(
          "div",
          { class: "phone" },
          deviceIcon(phones[0]),
          h("div", { class: "t" }, h("b", { text: v.title }), h("br"), h("span", { class: "note", text: v.subtitle })),
        ),
      );
    } else if (phones.length > 0) {
      if (note) parts.push(h("p", { class: "note", text: note }));
      parts.push(h("div", { class: "devices" }, ...phones.map(deviceCard)));
    } else {
      const text = err?.text ?? "Подключите iPhone кабелем";
      const card = h(
        "div",
        { class: "phone" },
        phoneIcon(),
        h("div", { class: "t" }, h("b", { text: "iPhone не подключён" }), h("br"), h("span", { class: "note", text })),
      );
      if (err?.code === "drivers_missing") {
        card.append(
          h("button", {
            class: "btn ghost small",
            text: "Открыть Microsoft Store",
            on: { click: () => void openUrl(APPLE_DEVICES_URL).catch(() => undefined) },
          }),
        );
      }
      parts.push(card);
    }
    if (phones.length > 0 && err) parts.push(h("p", { class: "error", text: err.text }));
    el.replaceChildren(...parts);
  }

  render();
  return { el, render };
}

/** Вопрос «тот же Apple ID?». Ответ пишется в `state.sameAppleId`. Общий для «iPhone» и «Готово». */
export function appleIdQuestion(onAnswer: () => void): { el: HTMLElement; render(): void } {
  const el = h("div", { class: "question" });
  function answer(same: boolean) {
    state.sameAppleId = same;
    render();
    onAnswer();
  }
  function render() {
    const yes = h("button", {
      class: "btn ghost small",
      text: "Да, тот же",
      attrs: { "aria-pressed": state.sameAppleId === true ? "true" : "false" },
      on: { click: () => answer(true) },
    });
    const no = h("button", {
      class: "btn ghost small",
      text: "Нет, другой",
      attrs: { "aria-pressed": state.sameAppleId === false ? "true" : "false" },
      on: { click: () => answer(false) },
    });
    const other = state.sameAppleId === false ? h("p", { text: otherIdText(state.run?.email ?? state.account?.email ?? null) }) : null;
    el.replaceChildren(h("b", { text: "На этом iPhone вход в тот же Apple ID?" }), h("div", { class: "opts" }, yes, no), ...(other ? [other] : []));
  }
  render();
  return { el, render };
}

/** Проверить Apple ID телефона `udid` и запомнить итог. Сбой — `unknown` (прежний вопрос). */
export async function checkAccount(udid: string): Promise<AccountMatchDto> {
  let m: AccountMatchDto = { kind: "unknown" };
  try {
    m = await call<AccountMatchDto>("phone_account", { udid });
  } catch {
    // прочитать не вышло: спросим человека
  }
  state.accountMatch = { udid, m };
  return m;
}

/** «Войти в Apple ID этого iPhone»: выход, ждущие задания убраны, вход с подставленным адресом. */
async function switchAccount(onRefused: (text: string) => void): Promise<void> {
  let res: SwitchDto = { prefill: null, dropped: [] };
  try {
    res = await call<SwitchDto>("switch_account");
  } catch (e) {
    const ui = asUiError(e);
    // идёт скачивание — выходить нельзя, остаёмся
    if (staysOnRefusal(ui)) {
      onRefused(ui.text);
      return;
    }
    // выйти не вышло — всё равно даём войти заново
  }
  // только то, что Rust действительно убрал: задание, уже взятое очередью, остаётся на виду
  for (const id of res.dropped) state.jobs.delete(id);
  const prefill = res.prefill;
  state.run = null;
  state.account = null;
  state.choice = null;
  state.accountMatch = null;
  state.sameAppleId = null;
  state.loginPrefill = prefill;
  state.switchedAccount = true;
  state.withPhone = true;
  refreshSidebar();
  go("login");
}

/** Блок про Apple ID на шаге iPhone: по итогу `phone_account` — ничего, предупреждение или вопрос. */
function accountBox(onChange: () => void): { el: HTMLElement; render(): void; check(): void } {
  const el = h("div", { attrs: { "aria-live": "polite" } });
  const question = appleIdQuestion(onChange);
  let pending: string | null = null;
  /** Отказ «Войти в Apple ID этого iPhone» (идёт скачивание). */
  let refusal: string | null = null;

  function render() {
    const t = targetPhone();
    const known = t && state.accountMatch?.udid === t.udid ? state.accountMatch.m : null;
    // телефон выбран, итог ещё не пришёл — без мелькания вопроса
    if (t && !known && pending === t.udid) {
      el.replaceChildren();
      return;
    }
    const b = accountBlock(known, state.account?.email ?? null);
    if (b.kind === "none") {
      el.replaceChildren();
    } else if (b.kind === "question") {
      question.render();
      el.replaceChildren(question.el);
    } else {
      const mine = h("button", {
        class: "btn ghost small",
        text: "Поставить с моего Apple ID",
        attrs: { "aria-pressed": state.sameAppleId === false ? "true" : "false" },
        on: {
          click: () => {
            state.sameAppleId = false;
            render();
            onChange();
          },
        },
      });
      const login = h("button", {
        class: "btn small",
        text: "Войти в Apple ID этого iPhone",
        on: {
          click: safe(async () => {
            refusal = null;
            await switchAccount((text) => {
              refusal = text;
              render();
            });
          }),
        },
      });
      const other = state.sameAppleId === false ? h("p", { text: otherIdText(state.account?.email ?? null) }) : null;
      const why = refusal ? h("p", { class: "error", attrs: { role: "alert" }, text: refusal }) : null;
      el.replaceChildren(h("div", { class: "question" }, h("b", { text: b.text }), h("div", { class: "opts" }, login, mine), ...(other ? [other] : []), ...(why ? [why] : [])));
    }
  }

  /** Новый выбранный телефон — новая проверка. Тот же — прошлый итог и ответ остаются. */
  function check() {
    const t = targetPhone();
    if (!t || !state.account || state.accountMatch?.udid === t.udid || pending === t.udid) {
      render();
      return;
    }
    const udid = t.udid;
    pending = udid;
    state.sameAppleId = null;
    render();
    onChange();
    void checkAccount(udid).then((m) => {
      if (pending === udid) pending = null;
      if (targetPhone()?.udid !== udid) return;
      state.sameAppleId = sameFromMatch(m);
      render();
      onChange();
    });
  }

  return { el, render, check };
}

register("phone", () => {
  const nextBtn = h("button", {
    class: "btn",
    text: "Дальше",
    on: {
      click: () => {
        if (!canProceed(state.phones, state.sameAppleId)) return;
        state.withPhone = true;
        go("choose");
      },
    },
  });
  const skipBtn = h("button", {
    class: "btn ghost",
    text: "Продолжить без iPhone",
    on: {
      click: () => {
        state.withPhone = false;
        go("choose");
      },
    },
  });
  const back = h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("accounts") } });

  const status = phoneStatus(() => renderNext());

  const account = accountBox(() => renderNext());

  function renderNext() {
    nextBtn.disabled = !canProceed(state.phones, state.sameAppleId);
    // с выбранным устройством идут через «Дальше»
    skipBtn.hidden = !showSkip(state.phones);
  }

  function onPhones() {
    status.render();
    account.check();
    renderNext();
  }

  renderNext();
  void refreshPhones().then(onPhones);

  const el = h(
    "div",
    { class: "body" },
    h("h2", { text: "Подключите iPhone" }),
    h("p", { class: "lead", text: "Кабелем. На телефоне нажмите «Доверять»." }),
    status.el,
    account.el,
    h("div", { class: "push" }, nextBtn, skipBtn, back),
  );
  return { el, onPhones };
});
