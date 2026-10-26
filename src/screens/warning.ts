import { go, register, safe, sessionReady, startScreen } from "../app";
import { h } from "../dom";

const KEY = "app-restore.warning-seen";

/** Видел ли человек предупреждение. Хранилище может быть недоступно — тогда покажем снова. */
export function warningSeen(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

function remember(): void {
  try {
    localStorage.setItem(KEY, "1");
  } catch {
    // не страшно: предупреждение покажется при следующем запуске
  }
}

register("warning", () => {
  const ok = h("button", {
    class: "btn",
    text: "Понятно",
    attrs: { autofocus: true },
    on: {
      click: safe(async () => {
        remember();
        ok.disabled = true;
        await sessionReady;
        go(await startScreen());
      }),
    },
  });
  const el = h(
    "div",
    { class: "body" },
    h("h2", { text: "Прежде чем начать" }),
    h("p", {
      class: "warn",
      text: "Программа работает с App Store не так, как официальное приложение Apple. Apple может временно заблокировать вход. Используйте Apple ID, к которому у вас есть доверенный телефон.",
    }),
    h("div", { class: "push" }, ok),
  );
  return { el };
});
