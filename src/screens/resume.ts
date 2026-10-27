// «Не закончили прошлый раз: N приложений — Продолжить / Забыть»: незавершённые задания
// прошлого запуска нынешнего Apple ID. Сами задания не начинаются — только по «Продолжить».
import { asUiError, call, go, safe, state } from "../app";
import { h } from "../dom";
import { jobInbox } from "../jobs";
import { errorText } from "../labels";
import { pendingText, resumedJob } from "../queue";
import type { PendingJobDto, ResumeDto } from "../types";

/** Баннер; пустой, пока не узнали, есть ли что продолжать, и когда продолжать нечего. */
export function resumeBanner(): HTMLElement {
  const box = h("div", { attrs: { "aria-live": "polite" } });
  let busy = false;

  function show(jobs: PendingJobDto[], error: string | null = null): void {
    if (!jobs.length) {
      box.replaceChildren();
      return;
    }
    const resume = h("button", {
      class: "btn small",
      text: "Продолжить",
      attrs: { type: "button" },
      on: {
        click: safe(async () => {
          if (busy) return;
          busy = true;
          try {
            const res = await call<ResumeDto>("resume_pending", { sameAppleId: state.sameAppleId ?? true });
            for (const p of res.jobs) jobInbox.register(state.jobs, resumedJob(p));
            if (res.run) state.run = res.run;
            go("done");
          } catch (e) {
            show(jobs, errorText(asUiError(e)));
          } finally {
            busy = false;
          }
        }),
      },
    });
    const forget = h("button", {
      class: "btn ghost small",
      text: "Забыть",
      attrs: { type: "button" },
      on: {
        click: safe(async () => {
          await call("forget_pending");
          box.replaceChildren();
        }),
      },
    });
    const text = h("span", { text: pendingText(jobs.length) });
    const msg = error ? h("p", { class: "error", attrs: { role: "alert" }, text: error }) : null;
    box.replaceChildren(h("div", { class: "banner" }, text, resume, forget), ...(msg ? [msg] : []));
  }

  void call<PendingJobDto[]>("pending_jobs")
    .then((jobs) => show(jobs))
    .catch(() => undefined);
  return box;
}
