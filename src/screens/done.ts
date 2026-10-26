import { go, openLibraryButton, register, safe, state, targetPhone } from "../app";
import { h } from "../dom";
import { askAppleIdOnDone, doneText, isFinished } from "../labels";
import { appleIdQuestion, jobRow, phoneStatus, retryJob } from "./phone";

register("done", () => {
  const big = h("div", { class: "big-ok" });
  const list = h("div", { class: "scroll" });
  const status = phoneStatus(() => onPhones());
  const phoneBox = h("div");
  // Шаг iPhone пропустили, телефон появился: спросить про Apple ID до итоговых подписей.
  const question = appleIdQuestion(() => render());
  const questionBox = h("div");

  const again = h("button", {
    class: "btn ghost",
    text: "Вернуть ещё",
    on: {
      click: () => {
        state.jobs = new Map();
        go("choose");
      },
    },
  });
  const back = h("button", { class: "link", text: "Назад", attrs: { type: "button" }, on: { click: () => go("choose") } });

  function render() {
    const jobs = [...state.jobs.values()];
    const n = jobs.filter((j) => j.state.kind === "installed").length;
    const finished = jobs.length > 0 && jobs.every((j) => isFinished(j.state));
    big.textContent = `${n} из ${jobs.length}`;
    big.className = n === jobs.length && finished ? "big-ok" : "big-ok partial";
    again.hidden = !finished;
    // Телефон показываем, пока есть что ставить и ставить некуда (или выбор не сделан).
    const waiting = jobs.some((j) => !isFinished(j.state));
    phoneBox.replaceChildren(...(waiting && !targetPhone() ? [status.el] : []));
    const ask = askAppleIdOnDone(state.withPhone, state.sameAppleId, state.phones);
    // после ответа вопрос остаётся на виду: можно передумать
    if (ask || (!state.withPhone && state.sameAppleId !== null)) questionBox.replaceChildren(question.el);
    // без ответа подпись нейтральная: «Установлено»
    const same = state.sameAppleId;
    const email = state.run?.email ?? null;
    list.replaceChildren(
      ...jobs.map((j) =>
        jobRow(
          j,
          doneText(j.state, j.fromSaved, same, email),
          safe(async () => {
            j.state = { kind: "queued" };
            render();
            await retryJob(j.track_id);
          }),
        ),
      ),
    );
  }

  /** Телефон перерисовывается только по событию `phones`, а не на каждый шаг задания. */
  function onPhones() {
    status.render();
    render();
  }

  render();
  const el = h(
    "div",
    { class: "body" },
    big,
    h("h2", { text: "Приложения на iPhone" }),
    phoneBox,
    questionBox,
    list,
    h(
      "div",
      { class: "saved" },
      "Копии лежат в папке «Мои приложения». Их можно поставить снова без App Store, даже когда Apple опять что-то сломает.",
    ),
    openLibraryButton("btn ghost small"),
    h("div", { class: "push" }, again, back),
  );
  return { el, onJob: render, onPhones };
});
