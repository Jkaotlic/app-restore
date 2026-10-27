// События `job` могут прийти раньше, чем экран узнал номер задания (`install_copy` ещё не
// вернулся). Последнее состояние таких номеров ждёт здесь и подставляется при регистрации.
import type { Job } from "./app";
import type { JobEvent } from "./types";

/** Состояние события — в задание; у «повтор через N с» — и когда пауза кончится. */
function put(job: Job, ev: JobEvent, at: number): void {
  job.state = ev.state;
  job.fromSaved = ev.from_saved;
  job.waitUntil = ev.state.kind === "waiting" ? at + ev.state.seconds * 1000 : undefined;
}

export class JobInbox {
  private early = new Map<number, { ev: JobEvent; at: number }>();

  constructor(private readonly cap = 64) {}

  /** Событие в известное задание; `false` — номер пока неизвестен, событие отложено. */
  apply(jobs: Map<number, Job>, ev: JobEvent, at = Date.now()): boolean {
    const job = jobs.get(ev.track_id);
    if (job) {
      put(job, ev, at);
      return true;
    }
    this.early.delete(ev.track_id);
    this.early.set(ev.track_id, { ev, at });
    while (this.early.size > this.cap) this.early.delete(this.early.keys().next().value as number);
    return false;
  }

  /** Новое задание экрана: в общий список, с отложенным состоянием, если оно уже пришло. */
  register(jobs: Map<number, Job>, job: Job): void {
    const early = this.early.get(job.track_id);
    this.early.delete(job.track_id);
    if (early) put(job, early.ev, early.at);
    jobs.set(job.track_id, job);
  }
}

export const jobInbox = new JobInbox();
