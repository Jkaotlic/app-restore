// Шаг «Apple ID»: все, кто входил в программу или чьи копии лежат в «Мои приложения».
import { asUiError, call, go, refreshSidebar, register, safe, state } from "../app";
import { accountCopiesText, accountStatus, afterUseAccount, pickStatusText, staysOnRefusal } from "../copies";
import { errorText } from "../labels";
import { h } from "../dom";
import type { AccountEntry, UseAccountDto } from "../types";

/** Аккаунт сменился: галочки, ответы про телефон и итог прошлого запуска не годятся. */
export function resetForAccount(dropped: number[]): void {
  for (const id of dropped) state.jobs.delete(id);
  state.choice = null;
  state.accountMatch = null;
  state.sameAppleId = null;
  state.run = null;
}

const sameId = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

register("accounts", () => {
  const list = h("div", { class: "scroll", attrs: { "aria-live": "polite" } });
  const msg = h("p", { class: "error", attrs: { role: "alert", "aria-live": "polite" } });
  msg.hidden = true;
  let accounts: AccountEntry[] | null = null;
  let busy: string | null = null;
  /** Проверка входа у Apple идёт дольше 3 с. */
  let slow = false;
  /** Ушли с экрана: по итогу смены никуда не переходим. */
  let gone = false;

  async function load(): Promise<void> {
    try {
      accounts = await call<AccountEntry[]>("list_accounts");
    } catch (e) {
      accounts = [];
      msg.textContent = errorText(asUiError(e));
      msg.hidden = false;
    }
    render();
  }

  async function pick(a: AccountEntry): Promise<void> {
    if (busy) return;
    busy = a.email;
    slow = false;
    msg.hidden = true;
    render();
    const timer = setTimeout(() => {
      if (busy !== a.email) return;
      slow = true;
      render();
    }, 3000);
    let res: UseAccountDto = { restored: false, account: state.account, prefill: a.email, dropped: [], error: null };
    try {
      res = await call<UseAccountDto>("use_account", { email: a.email });
    } catch (e) {
      const ui = asUiError(e);
      // идёт скачивание — ничего не меняли, остаёмся; прочее — войдём с адресом
      if (staysOnRefusal(ui)) res = { ...res, prefill: null, error: ui };
    }
    clearTimeout(timer);
    busy = null;
    slow = false;
    const next = afterUseAccount(res);
    if (next === "stay") {
      // нынешний вход не сохранился — ничего не меняли
      msg.textContent = res.error ? errorText(res.error) : "";
      msg.hidden = false;
      render();
      return;
    }
    const navigate = !gone;
    // вход ipatool мог остаться прежним (откат): программа в том аккаунте, который вернул Rust
    const now = res.account?.email ?? null;
    if (state.account && (!now || !sameId(state.account.email, now))) resetForAccount(res.dropped);
    else for (const id of res.dropped) state.jobs.delete(id);
    state.addingAccount = false;
    state.account = res.account;
    refreshSidebar();
    if (next === "login") state.loginPrefill = res.prefill ?? a.email;
    // ушли с экрана, пока проверяли вход: состояние обновили, но не уводим
    if (!navigate) return;
    // Apple просит войти заново (или снимка нет): вход с подставленным адресом, пароль не храним
    go(next);
  }

  async function forget(a: AccountEntry): Promise<void> {
    if (busy) return;
    let dropped: number[] = [];
    msg.hidden = true;
    try {
      dropped = await call<number[]>("forget_account", { email: a.email });
    } catch (e) {
      // выйти не вышло — ничего не забыли, вход прежний
      msg.textContent = errorText(asUiError(e));
      msg.hidden = false;
      return;
    }
    if (state.account && sameId(state.account.email, a.email)) {
      resetForAccount(dropped);
      state.account = null;
      refreshSidebar();
    }
    await load();
  }

  function row(a: AccountEntry): HTMLElement {
    const letter = a.email.match(/[\p{L}\p{N}]/u)?.[0]?.toLocaleUpperCase("ru") ?? "?";
    const lines = [
      h("small", { class: "muted", text: accountCopiesText(a.copies) }),
      h("small", { class: a.saved ? "" : "muted", text: busy === a.email ? pickStatusText(slow) : accountStatus(a) }),
    ];
    const main = h(
      "button",
      {
        class: "pick",
        attrs: { type: "button", "aria-current": a.current ? "true" : undefined, disabled: busy !== null },
        on: { click: safe(() => pick(a)) },
      },
      h("span", { class: "ico acct", text: letter }),
      h("span", { class: "t" }, h("b", { text: a.email }), ...lines),
    );
    const forgetBtn = a.saved
      ? h("button", { class: "link", text: "Забыть", attrs: { type: "button", disabled: busy !== null }, on: { click: safe(() => forget(a)) } })
      : null;
    return h("div", { class: a.current ? "app on" : "app" }, main, forgetBtn);
  }

  function render(): void {
    if (accounts === null) {
      list.replaceChildren(h("p", { class: "loading" }, "Открываем…", h("span", { class: "spin", attrs: { "aria-hidden": "true" } })));
      return;
    }
    list.replaceChildren(...accounts.map(row));
  }

  const add = h("button", {
    class: "btn",
    text: "Добавить Apple ID",
    on: {
      click: () => {
        state.loginPrefill = null;
        state.addingAccount = true;
        go("login");
      },
    },
  });

  render();
  void load();
  const el = h(
    "div",
    { class: "body" },
    h("h2", { text: "Выберите Apple ID" }),
    h("p", { class: "lead", text: "Тот, на котором вы раньше скачивали эти приложения." }),
    msg,
    list,
    h("div", { class: "push" }, add),
  );
  return {
    el,
    leave: () => {
      gone = true;
    },
  };
});
