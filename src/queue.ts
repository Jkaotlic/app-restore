// Очередь на «Готово»: общий прогресс с оценкой времени по скорости скачивания и
// незавершённые задания прошлого запуска («В прошлый раз не закончили»).
import type { Job } from "./app";
import type { PendingJobDto, RowDto } from "./types";

/** Доля нового замера в средней скорости: замеры дважды в секунду, средняя — за ~3 с. */
const ALPHA = 0.2;
/** Перерыв между замерами длиннее — сон Mac или пауза: замер не в счёт. */
const GAP_MS = 30_000;

/** Скользящая средняя скорости скачивания (байт/с) по всем скачиваниям подряд. */
export class SpeedMeter {
  bps: number | null = null;
  private last = new Map<number, { bytes: number; at: number }>();

  /** Задание `id` скачало `bytes` к моменту `at` (мс). */
  sample(id: number, bytes: number, at: number): void {
    const prev = this.last.get(id);
    this.last.set(id, { bytes, at });
    if (!prev) return;
    const dt = at - prev.at;
    const db = bytes - prev.bytes;
    // то же время, скачивание началось заново или долгий перерыв — только новая точка отсчёта
    if (dt <= 0 || db < 0 || dt > GAP_MS) return;
    const now = (db * 1000) / dt;
    this.bps = this.bps === null ? now : this.bps + ALPHA * (now - this.bps);
  }

  /** Скачивание `id` кончилось (или прервано): следующий замер — с новой точки отсчёта. */
  stop(id: number): void {
    this.last.delete(id);
  }
}

export const speedMeter = new SpeedMeter();

/** Ещё будет скачиваться: ждёт, качается, ждёт Apple или «на паузе». */
function toDownload(j: Job): boolean {
  const k = j.state.kind;
  if (k === "downloading" || k === "waiting" || k === "paused") return true;
  // запись каталога с копией в «Мои приложения» ставится без скачивания (`plan_start`)
  return k === "queued" && !(j.row.source === "removed" && j.row.has_saved_copy);
}

/** Сколько байт осталось скачать; размер хоть одного неизвестен — `null` (оценку не показываем). */
export function remainingBytes(jobs: Job[]): number | null {
  let sum = 0;
  for (const j of jobs.filter(toDownload)) {
    const size = j.row.size_bytes;
    if (size === null || size === undefined) return null;
    const got = j.state.kind === "downloading" ? j.state.bytes : 0;
    sum += Math.max(0, size - got);
  }
  return sum;
}

/** Минуты до конца скачиваний, вверх; неизвестно или нечего ждать — `null`. */
export function etaMinutes(remaining: number | null, bps: number | null): number | null {
  if (remaining === null || remaining <= 0 || bps === null || bps <= 0) return null;
  return Math.ceil(remaining / bps / 60);
}

/** «3 из 12 · осталось ≈ 4 мин»; на паузе — «· на паузе»; оценки нет — просто «3 из 12». */
export function overallText(done: number, total: number, eta: number | null, paused: boolean): string {
  const base = `${done} из ${total}`;
  if (paused) return `${base} · на паузе`;
  return eta === null ? base : `${base} · осталось ≈ ${eta} мин`;
}

/** «1 приложение», «3 приложения», «5 приложений». */
export function appsCount(n: number): string {
  const d = n % 10;
  const dd = n % 100;
  const word = d === 1 && dd !== 11 ? "приложение" : d >= 2 && d <= 4 && (dd < 12 || dd > 14) ? "приложения" : "приложений";
  return `${n} ${word}`;
}

export function pendingText(n: number): string {
  return `В прошлый раз не закончили: ${appsCount(n)}`;
}

/** Задание «Продолжить»: строки «Выбора» у него нет — значок строится по имени. */
export function resumedJob(p: PendingJobDto): Job {
  const row: RowDto = {
    track_id: p.track_id,
    bundle_id: "",
    name: p.name,
    version: "",
    removed_from_ru: false,
    has_saved_copy: false,
    owned: true,
    source: "purchase",
    letter: null,
    color: null,
    installed_version: null,
    available_version: null,
    install_state: "unknown",
    checked: false,
    ipad_only: false,
    unknown_account: false,
    aliases: [],
    category: null,
    size_bytes: null,
    other_account: false,
    purchaser_masked: null,
    min_ios: null,
  };
  return { track_id: p.track_id, name: p.name, row, state: { kind: "queued" }, fromSaved: null, downloadOnly: p.download_only };
}
