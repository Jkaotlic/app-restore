import { call, go, openLibraryButton, register, safe, state, targetPhone, type Job } from "../app";
import { h } from "../dom";
import { askAppleIdOnDone, doneText, isFinished } from "../labels";
import { etaMinutes, overallText, remainingBytes, speedMeter } from "../queue";
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
  // «Пауза»: новые не начинаются, идущее скачивание обрывается и начнётся заново по «Продолжить»
  const pauseBtn = h("button", {
    class: "btn ghost small",
    attrs: { type: "button" },
    on: {
      click: safe(async () => {
        const next = !state.paused;
        state.paused = next;
        render();
        await call(next ? "pause_queue" : "resume_queue");
      }),
    },
  });
  const overall = h("div", { class: "overall", attrs: { role: "status" } }, big, pauseBtn);

  function render() {
    const jobs = [...state.jobs.values()];
    // «Только скачать»: сохранённое и есть итог
    const done = (j: Job) => j.state.kind === "installed" || (!!j.downloadOnly && j.state.kind === "saved");
    const n = jobs.filter(done).length;
    const finished = jobs.length > 0 && jobs.every((j) => isFinished(j.state, j.downloadOnly));
    // «3 из 12 · осталось ≈ 4 мин»: оценка — по средней скорости скачивания, неизвестна — не показываем
    const eta = etaMinutes(remainingBytes(jobs), speedMeter.bps);
    const [count, rest] = overallText(n, jobs.length, eta, state.paused && !finished).split(" · ");
    big.replaceChildren(count, ...(rest ? [h("span", { class: "eta", text: ` · ${rest}` })] : []));
    big.className = n === jobs.length && finished ? "big-ok" : "big-ok partial";
    again.hidden = !finished;
    pauseBtn.hidden = finished;
    pauseBtn.textContent = state.paused ? "Продолжить" : "Пауза";
    // Телефон показываем, пока есть что ставить и ставить некуда (или выбор не сделан).
    const waiting = jobs.some((j) => !isFinished(j.state, j.downloadOnly));
    phoneBox.replaceChildren(...(waiting && !targetPhone() ? [status.el] : []));
    const ask = askAppleIdOnDone(state.withPhone, state.sameAppleId, state.phones);
    // после ответа вопрос остаётся на виду: можно передумать
    if (ask || (!state.withPhone && state.sameAppleId !== null)) questionBox.replaceChildren(question.el);
    // без ответа подпись нейтральная: «Установлено»
    const same = state.sameAppleId;
    const email = state.run?.email ?? null;
    list.replaceChildren(
      ...jobs.map((j) => {
        const row = jobRow(
          j,
          doneText(j.state, j.fromSaved, same, email, j.downloadOnly),
          safe(async () => {
            j.state = { kind: "queued" };
            render();
            await retryJob(j.track_id);
          }),
          j.downloadOnly,
        );
        if (j.downloadOnly && j.state.kind === "saved") row.querySelector(".right")?.append(openLibraryButton("btn ghost small", "Открыть папку"));
        return row;
      }),
    );
  }

  /** Телефон перерисовывается только по событию `phones`, а не на каждый шаг задания. */
  function onPhones() {
    status.render();
    render();
  }

  render();
  // пауза могла остаться с прошлого захода на «Готово»
  void call<boolean>("queue_paused")
    .then((p) => {
      state.paused = p;
      render();
    })
    .catch(() => undefined);
  const el = h(
    "div",
    { class: "body" },
    overall,
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
