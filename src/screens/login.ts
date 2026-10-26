import { call, go, refreshSidebar, register, safe, state, targetPhone } from "../app";
import { h } from "../dom";
import { afterLoginScreen, sameFromMatch } from "../labels";
import type { LoginDto } from "../types";
import { resetForAccount } from "./accounts";
import { checkAccount } from "./phone";

const DIGITS = 6;

register("login", () => {
  const email = h("input", {
    class: "input",
    attrs: { type: "email", autocomplete: "username", spellcheck: "false", autofocus: true, placeholder: "name@example.com" },
  });
  const password = h("input", { class: "input", attrs: { type: "password", autocomplete: "current-password" } });
  // после «Войти в Apple ID этого iPhone» — адрес телефона (в логи не попадает)
  if (state.loginPrefill) email.value = state.loginPrefill;
  else if (state.account && !state.addingAccount) email.value = state.account.email;

  const boxes = Array.from({ length: DIGITS }, (_, i) =>
    h("input", { attrs: { inputmode: "numeric", maxlength: "1", autocomplete: "one-time-code", "aria-label": `Цифра ${i + 1} из ${DIGITS}` } }),
  );
  const codeLabel = h("span", { text: "Код с iPhone" });
  const codeBlock = h("div", { class: "field" }, codeLabel, h("div", { class: "code" }, ...boxes));
  codeBlock.hidden = true;

  const message = h("p", { class: "error", attrs: { role: "alert", "aria-live": "polite" } });
  message.hidden = true;
  const button = h("button", { class: "btn", text: "Войти", attrs: { type: "submit" } });

  const code = () => boxes.map((b) => b.value).join("");

  function fill(from: number, digits: string) {
    let i = from;
    for (const d of digits.replace(/\D/g, "")) {
      if (i >= DIGITS) break;
      boxes[i++].value = d;
    }
    boxes[Math.min(i, DIGITS - 1)].focus();
  }

  boxes.forEach((b, i) => {
    b.addEventListener("input", () => {
      const v = b.value.replace(/\D/g, "");
      b.value = "";
      if (v) fill(i, v);
    });
    b.addEventListener("paste", (e: ClipboardEvent) => {
      e.preventDefault();
      fill(i, e.clipboardData?.getData("text") ?? "");
    });
    b.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Backspace" && !b.value && i > 0) {
        boxes[i - 1].value = "";
        boxes[i - 1].focus();
        e.preventDefault();
      }
    });
  });

  function say(text: string, isError: boolean) {
    message.textContent = text;
    message.className = isError ? "error" : "note";
    message.hidden = false;
  }

  async function submit(e: Event) {
    e.preventDefault();
    if (!email.value.trim() || !password.value) {
      say("Введите Apple ID и пароль.", true);
      return;
    }
    const withCode = !codeBlock.hidden;
    if (withCode && code().length < DIGITS) {
      say("Введите все 6 цифр кода.", true);
      return;
    }
    button.disabled = true;
    button.textContent = "Входим…";
    try {
      const res = await call<LoginDto>("login", { email: email.value.trim(), password: password.value, code: withCode ? code() : null });
      password.value = "";
      const acc = { email: res.email, name: res.name };
      for (const id of res.dropped) state.jobs.delete(id);
      // другой Apple ID: галочки и ответы прошлого не годятся (ждущие задания остаются)
      if (state.account && state.account.email.toLowerCase() !== acc.email.toLowerCase()) resetForAccount([]);
      state.addingAccount = false;
      state.account = acc;
      state.loginPrefill = null;
      state.accountMatch = null;
      refreshSidebar();
      const switched = state.switchedAccount;
      state.switchedAccount = false;
      const phone = targetPhone();
      // права и строки «Выбора» уже по новому аккаунту; телефон тот же — проверяем совпадение
      const m = switched && phone ? await checkAccount(phone.udid) : null;
      if (m) state.sameAppleId = sameFromMatch(m);
      go(afterLoginScreen(switched, m));
      return;
    } catch (err) {
      const ui = err as { code: string; text: string };
      if (ui.code === "auth_code_required") {
        codeBlock.hidden = false;
        boxes.forEach((b) => (b.value = ""));
        say(ui.text, false);
        boxes[0].focus();
      } else {
        say(ui.text, true);
        if (ui.code === "code_rejected") {
          boxes.forEach((b) => (b.value = ""));
          boxes[0].focus();
        }
      }
    }
    button.disabled = false;
    button.textContent = "Войти";
  }

  const form = h(
    "form",
    { class: "body", on: { submit: safe(submit) } },
    h("h2", { text: "Войдите в Apple ID" }),
    h("p", { class: "lead", text: "Тот, на котором вы раньше скачивали эти приложения." }),
    h("label", { class: "field" }, h("span", { text: "Apple ID" }), email),
    h("label", { class: "field" }, h("span", { text: "Пароль" }), password),
    codeBlock,
    message,
    h(
      "div",
      { class: "push" },
      button,
      h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("accounts") } }),
      h("span", { class: "note", text: "Пароль не сохраняется на компьютере." }),
    ),
  );
  return { el: form };
});
